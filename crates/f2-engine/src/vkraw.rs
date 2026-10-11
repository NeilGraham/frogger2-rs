//! wgpu on a Vulkan driver the process links statically, with presentation
//! through the driver's own swapchain (feature `ps5-vulkan`).
//!
//! On the PS5 the Vulkan driver (Mesa RADV with a PS5 winsys, from the
//! PS5_Vulkan project) is a static archive linked into the title: there is no
//! loader and no `libvulkan.so` for wgpu-hal to `dlopen`, and no window system.
//! This module therefore
//!
//! 1. builds an [`ash::Entry`] from any `vkGetInstanceProcAddr` (on the console:
//!    the archive's `vk_icdGetInstanceProcAddr`, feature `ps5-static-driver`; on
//!    a desktop: [`ash::Entry::load`], which is how the host tests exercise the
//!    exact same code path), creates the `VkInstance` itself and hands it to
//!    wgpu with `wgpu_hal::vulkan::Instance::from_raw` + `wgpu::Instance::from_hal`
//!    ([`GpuContext::from_vulkan_entry`]);
//! 2. presents with [`VkPresenter`]: a `VkSwapchainKHR` created with ash on
//!    wgpu's own `VkDevice`/`VkQueue`, on a `VkSurfaceKHR` from `VK_KHR_display`
//!    (RADV's VideoOut WSI on the PS5; [`VkPresenter::new_display`]) or any other
//!    surface ([`VkPresenter::from_vk_surface`]). wgpu-hal cannot wrap a foreign
//!    `VkSurfaceKHR` (its constructor is crate-private), so the swapchain images
//!    are imported as ordinary wgpu textures (`create_texture_from_hal`) that the
//!    existing [`crate::Renderer`] draws into directly; layout transitions to and
//!    from `PRESENT_SRC_KHR` and the acquire/present semaphores are added around
//!    wgpu's single queue submission.
//!
//! Everything here is plain Vulkan through ash, so it compiles and runs on any
//! host with a Vulkan driver; only the statically linked entry point is PS5
//! specific.

use crate::renderer::{EngineError, GpuContext};
use ash::vk;
use std::ffi::{c_char, CStr};
use wgpu::hal;
use wgpu::hal::api::Vulkan;

pub use ash;

fn err(e: impl std::fmt::Display) -> EngineError {
    EngineError(e.to_string())
}

/// `vkGetInstanceProcAddr` of a statically linked Vulkan driver.
pub type GetInstanceProcAddr = unsafe extern "system" fn(vk::Instance, *const c_char) -> vk::PFN_vkVoidFunction;

/// An [`ash::Entry`] over a driver's `vkGetInstanceProcAddr` (no loader, no dlopen).
///
/// # Safety
/// `gipa` must be a valid `vkGetInstanceProcAddr` for the life of the process.
pub unsafe fn entry_from_gipa(gipa: GetInstanceProcAddr) -> ash::Entry {
    unsafe { ash::Entry::from_static_fn(ash::StaticFn { get_instance_proc_addr: gipa }) }
}

/// The PS5 RADV archive's entry point (`radv_instance.c`). Mesa built without a
/// loader exports it as the ICD interface; the PS5_Vulkan smoke test, vkQuake
/// and RetroArch resolve everything through it.
#[cfg(feature = "ps5-static-driver")]
mod ps5_driver {
    use super::*;
    extern "system" {
        pub fn vk_icdGetInstanceProcAddr(instance: vk::Instance, name: *const c_char) -> vk::PFN_vkVoidFunction;
    }
}

/// The statically linked PS5 driver as an [`ash::Entry`].
#[cfg(feature = "ps5-static-driver")]
pub fn ps5_entry() -> ash::Entry {
    // SAFETY: the symbol is the driver archive's, linked into the title.
    unsafe { entry_from_gipa(ps5_driver::vk_icdGetInstanceProcAddr) }
}

impl GpuContext {
    /// Creates a wgpu context (Vulkan only) on the driver behind `entry`.
    ///
    /// The instance gets the extensions wgpu-hal would enable itself plus
    /// `extra_instance_extensions` that the driver offers (e.g.
    /// `VK_KHR_display` for [`VkPresenter::new_display`]). Adapter and device
    /// are requested exactly like [`GpuContext::with_instance`], so rendering is
    /// identical to the normal (loader) path.
    pub fn from_vulkan_entry(entry: ash::Entry, extra_instance_extensions: &[&'static CStr]) -> Result<GpuContext, EngineError> {
        let flags = wgpu::InstanceFlags::from_build_config().with_env();
        // SAFETY: plain Vulkan instance creation; the instance is handed to
        // wgpu-hal, which takes ownership (drop_callback = None).
        let hal_instance = unsafe {
            let version = entry.try_enumerate_instance_version().map_err(err)?.unwrap_or(vk::API_VERSION_1_0);
            // wgpu-hal's ceiling (it audits behaviour up to Vulkan 1.3).
            let api_version = if version < vk::API_VERSION_1_1 { vk::API_VERSION_1_0 } else { version.min(vk::API_VERSION_1_3) };
            let mut extensions = hal::vulkan::Instance::desired_extensions(&entry, version, flags).map_err(err)?;
            let available = entry.enumerate_instance_extension_properties(None).map_err(err)?;
            for &ext in extra_instance_extensions {
                let offered = available.iter().any(|p| p.extension_name_as_c_str() == Ok(ext));
                if offered && !extensions.contains(&ext) {
                    extensions.push(ext);
                }
            }
            let app_info = vk::ApplicationInfo::default()
                .application_name(c"frogger2-rs")
                .application_version(1)
                .engine_name(c"wgpu-hal")
                .engine_version(2)
                .api_version(api_version);
            let ext_ptrs: Vec<*const c_char> = extensions.iter().map(|e| e.as_ptr()).collect();
            let info = vk::InstanceCreateInfo::default().application_info(&app_info).enabled_extension_names(&ext_ptrs);
            let raw = entry.create_instance(&info, None).map_err(|e| err(format!("vkCreateInstance: {e}")))?;
            hal::vulkan::Instance::from_raw(entry, raw, api_version, 0, None, extensions, flags, Default::default(), false, None).map_err(err)?
        };
        // SAFETY: a freshly created hal instance.
        let instance = unsafe { wgpu::Instance::from_hal::<Vulkan>(hal_instance) };
        let ctx = GpuContext::with_instance(instance, None)?;
        let info = ctx.adapter.get_info();
        eprintln!("f2-engine: Vulkan via raw vkGetInstanceProcAddr: {} ({}, {})", info.name, info.driver, info.driver_info);
        Ok(ctx)
    }

    /// The PS5 context: the statically linked RADV, with `VK_KHR_display` for
    /// [`VkPresenter::new_display`].
    #[cfg(feature = "ps5-static-driver")]
    pub fn new_ps5() -> Result<GpuContext, EngineError> {
        GpuContext::from_vulkan_entry(ps5_entry(), &[ash::khr::display::NAME])
    }

    /// ash handles of wgpu's Vulkan objects (entry, instance, physical device,
    /// device, queue family, queue).
    pub fn vulkan_handles(&self) -> Result<VkHandles, EngineError> {
        // SAFETY: only clones the handles; nothing is destroyed through them.
        unsafe {
            let inst = self.instance.as_hal::<Vulkan>().ok_or_else(|| err("not a Vulkan instance"))?;
            let shared = inst.shared_instance();
            let dev = self.device.as_hal::<Vulkan>().ok_or_else(|| err("not a Vulkan device"))?;
            Ok(VkHandles {
                entry: shared.entry().clone(),
                instance: shared.raw_instance().clone(),
                physical_device: dev.raw_physical_device(),
                device: dev.raw_device().clone(),
                queue_family_index: dev.queue_family_index(),
                queue: dev.raw_queue(),
            })
        }
    }
}

/// Raw Vulkan handles owned by wgpu (never destroy them).
#[derive(Clone)]
pub struct VkHandles {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue_family_index: u32,
    pub queue: vk::Queue,
}

fn wgpu_format(f: vk::Format) -> Option<wgpu::TextureFormat> {
    use wgpu::TextureFormat as T;
    Some(match f {
        vk::Format::B8G8R8A8_UNORM => T::Bgra8Unorm,
        vk::Format::B8G8R8A8_SRGB => T::Bgra8UnormSrgb,
        vk::Format::R8G8B8A8_UNORM => T::Rgba8Unorm,
        vk::Format::R8G8B8A8_SRGB => T::Rgba8UnormSrgb,
        vk::Format::A2B10G10R10_UNORM_PACK32 => T::Rgb10a2Unorm,
        _ => return None,
    })
}

/// One acquired swapchain image: draw into `view` (size [`VkPresenter::size`],
/// format [`VkPresenter::format`]), then [`VkPresenter::present`].
pub struct VkFrame {
    image_index: u32,
    slot: usize,
}

struct Slot {
    acquired: vk::Semaphore,
    last_submit: Option<wgpu::SubmissionIndex>,
}

/// A swapchain on wgpu's device, presented outside wgpu (see the module docs).
pub struct VkPresenter {
    h: VkHandles,
    device: wgpu::Device,
    surface_fn: ash::khr::surface::Instance,
    swapchain_fn: ash::khr::swapchain::Device,
    surface: vk::SurfaceKHR,
    owns_surface: bool,
    swapchain: vk::SwapchainKHR,
    format: wgpu::TextureFormat,
    extent: vk::Extent2D,
    textures: Vec<(wgpu::Texture, wgpu::TextureView)>,
    /// Signalled by the render submission, waited by the present; one per image.
    rendered: Vec<vk::Semaphore>,
    slots: Vec<Slot>,
    next_slot: usize,
}

impl VkPresenter {
    /// Presents to the first `VK_KHR_display` display in its first (preferred)
    /// mode on plane 0: on the PS5, RADV's VideoOut swapchain (3840x2160 when
    /// the TV takes it). The context must come from
    /// [`GpuContext::from_vulkan_entry`] with `VK_KHR_display` (e.g.
    /// [`GpuContext::new_ps5`]).
    pub fn new_display(ctx: &GpuContext) -> Result<VkPresenter, EngineError> {
        let h = ctx.vulkan_handles()?;
        let display_fn = ash::khr::display::Instance::new(&h.entry, &h.instance);
        // SAFETY: queries and surface creation on wgpu's live instance.
        let (surface, extent) = unsafe {
            let displays = display_fn.get_physical_device_display_properties(h.physical_device).map_err(|e| err(format!("VK_KHR_display: {e}")))?;
            let display = displays.first().ok_or_else(|| err("VK_KHR_display: no display"))?;
            let modes = display_fn.get_display_mode_properties(h.physical_device, display.display).map_err(err)?;
            let mode = modes.first().ok_or_else(|| err("VK_KHR_display: display has no modes"))?;
            let planes = display_fn.get_physical_device_display_plane_properties(h.physical_device).map_err(err)?;
            let plane = (0..planes.len() as u32)
                .find(|&p| display_fn.get_display_plane_supported_displays(h.physical_device, p).map(|d| d.contains(&display.display)).unwrap_or(false))
                .unwrap_or(0);
            let caps = display_fn.get_display_plane_capabilities(h.physical_device, mode.display_mode, plane).map_err(err)?;
            let alpha = if caps.supported_alpha.contains(vk::DisplayPlaneAlphaFlagsKHR::OPAQUE) {
                vk::DisplayPlaneAlphaFlagsKHR::OPAQUE
            } else {
                vk::DisplayPlaneAlphaFlagsKHR::GLOBAL
            };
            let extent = mode.parameters.visible_region;
            let info = vk::DisplaySurfaceCreateInfoKHR::default()
                .display_mode(mode.display_mode)
                .plane_index(plane)
                .transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
                .global_alpha(1.0)
                .alpha_mode(alpha)
                .image_extent(extent);
            (display_fn.create_display_plane_surface(&info, None).map_err(|e| err(format!("vkCreateDisplayPlaneSurfaceKHR: {e}")))?, extent)
        };
        // SAFETY: the surface was created on this instance and is owned here.
        unsafe { Self::new(h, ctx.device.clone(), surface, true, extent.width, extent.height) }
    }

    /// Presents to an existing `VkSurfaceKHR` of `ctx`'s instance (not taken
    /// over: destroy it after the presenter). `width`/`height` are used when the
    /// surface does not dictate its extent.
    ///
    /// # Safety
    /// `surface` must be a valid surface of `ctx`'s `VkInstance`.
    pub unsafe fn from_vk_surface(ctx: &GpuContext, surface: vk::SurfaceKHR, width: u32, height: u32) -> Result<VkPresenter, EngineError> {
        let h = ctx.vulkan_handles()?;
        unsafe { Self::new(h, ctx.device.clone(), surface, false, width, height) }
    }

    unsafe fn new(
        h: VkHandles,
        device: wgpu::Device,
        surface: vk::SurfaceKHR,
        owns_surface: bool,
        width: u32,
        height: u32,
    ) -> Result<VkPresenter, EngineError> {
        let surface_fn = ash::khr::surface::Instance::new(&h.entry, &h.instance);
        let swapchain_fn = ash::khr::swapchain::Device::new(&h.instance, &h.device);
        let mut p = VkPresenter {
            h,
            device,
            surface_fn,
            swapchain_fn,
            surface,
            owns_surface,
            swapchain: vk::SwapchainKHR::null(),
            format: wgpu::TextureFormat::Bgra8Unorm,
            extent: vk::Extent2D { width, height },
            textures: Vec::new(),
            rendered: Vec::new(),
            slots: Vec::new(),
            next_slot: 0,
        };
        unsafe { p.create_swapchain()? };
        Ok(p)
    }

    unsafe fn create_swapchain(&mut self) -> Result<(), EngineError> {
        let h = self.h.clone();
        unsafe {
            let supported = self.surface_fn.get_physical_device_surface_support(h.physical_device, h.queue_family_index, self.surface).map_err(err)?;
            if !supported {
                return Err(err("wgpu's queue family cannot present to this surface"));
            }
            let caps = self.surface_fn.get_physical_device_surface_capabilities(h.physical_device, self.surface).map_err(err)?;
            let formats = self.surface_fn.get_physical_device_surface_formats(h.physical_device, self.surface).map_err(err)?;
            // The original renders in gamma space: prefer a non-sRGB format.
            let mut known: Vec<(vk::SurfaceFormatKHR, wgpu::TextureFormat)> = formats.iter().filter_map(|f| wgpu_format(f.format).map(|w| (*f, w))).collect();
            known.sort_by_key(|(_, w)| w.is_srgb());
            let &(sformat, wformat) = known.first().ok_or_else(|| err(format!("no usable surface format in {formats:?}")))?;
            let extent = if caps.current_extent.width != u32::MAX {
                caps.current_extent
            } else {
                vk::Extent2D {
                    width: self.extent.width.clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                    height: self.extent.height.clamp(caps.min_image_extent.height, caps.max_image_extent.height),
                }
            };
            let mut count = caps.min_image_count.max(3);
            if caps.max_image_count != 0 {
                count = count.min(caps.max_image_count);
            }
            let alpha = [
                vk::CompositeAlphaFlagsKHR::OPAQUE,
                vk::CompositeAlphaFlagsKHR::INHERIT,
                vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
                vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
            ]
            .into_iter()
            .find(|a| caps.supported_composite_alpha.contains(*a))
            .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE);
            let old = self.swapchain;
            let info = vk::SwapchainCreateInfoKHR::default()
                .surface(self.surface)
                .min_image_count(count)
                .image_format(sformat.format)
                .image_color_space(sformat.color_space)
                .image_extent(extent)
                .image_array_layers(1)
                .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST)
                .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                .pre_transform(caps.current_transform)
                .composite_alpha(alpha)
                .present_mode(vk::PresentModeKHR::FIFO)
                .clipped(true)
                .old_swapchain(old);
            let swapchain = self.swapchain_fn.create_swapchain(&info, None).map_err(|e| err(format!("vkCreateSwapchainKHR: {e}")))?;
            self.destroy_swapchain_objects();
            if old != vk::SwapchainKHR::null() {
                self.swapchain_fn.destroy_swapchain(old, None);
            }
            self.swapchain = swapchain;
            self.format = wformat;
            self.extent = extent;
            let images = self.swapchain_fn.get_swapchain_images(swapchain).map_err(err)?;
            for &image in &images {
                let tex = import_image(&self.device, image, wformat, extent);
                let view = tex.create_view(&Default::default());
                self.textures.push((tex, view));
                self.rendered.push(self.h.device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).map_err(err)?);
            }
            for _ in 0..images.len() {
                let acquired = self.h.device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).map_err(err)?;
                self.slots.push(Slot { acquired, last_submit: None });
            }
            self.next_slot = 0;
        }
        Ok(())
    }

    unsafe fn destroy_swapchain_objects(&mut self) {
        unsafe {
            let _ = self.h.device.queue_wait_idle(self.h.queue);
            self.textures.clear();
            for s in self.rendered.drain(..) {
                self.h.device.destroy_semaphore(s, None);
            }
            for s in self.slots.drain(..) {
                self.h.device.destroy_semaphore(s.acquired, None);
            }
        }
    }

    /// Swapchain image format (create the [`crate::Renderer`] with it).
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Swapchain image size in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.extent.width, self.extent.height)
    }

    /// Waits for the next swapchain image.
    pub fn acquire(&mut self, ctx: &GpuContext) -> Result<(VkFrame, &wgpu::TextureView), EngineError> {
        let slot = self.next_slot;
        self.next_slot = (self.next_slot + 1) % self.slots.len();
        // The slot's acquire semaphore is free again once the submission that
        // waited on it has completed.
        if let Some(idx) = self.slots[slot].last_submit.take() {
            ctx.device.poll(wgpu::PollType::Wait { submission_index: Some(idx), timeout: None }).map_err(err)?;
        }
        // SAFETY: swapchain and semaphore are live and owned here.
        let (index, _suboptimal) = unsafe {
            self.swapchain_fn
                .acquire_next_image(self.swapchain, u64::MAX, self.slots[slot].acquired, vk::Fence::null())
                .map_err(|e| err(format!("vkAcquireNextImageKHR: {e}")))?
        };
        Ok((VkFrame { image_index: index, slot }, &self.textures[index as usize].1))
    }

    /// Submits `encoder` (which must draw the whole frame into the acquired
    /// image's view, e.g. with [`crate::Renderer::encode`]) and presents it.
    pub fn present(&mut self, ctx: &GpuContext, frame: VkFrame, encoder: wgpu::CommandEncoder) -> Result<(), EngineError> {
        let image = self.raw_image(frame.image_index)?;
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        // Before wgpu's commands: whatever the presentation engine left ->
        // COLOR_ATTACHMENT_OPTIMAL, the layout wgpu's tracker believes the
        // image is in (its RENDER_ATTACHMENT state; UNINITIALIZED on first use,
        // from which wgpu itself transitions from UNDEFINED).
        let pre = self.barrier_encoder(
            ctx,
            image,
            range,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
        )?;
        // After: COLOR_ATTACHMENT_OPTIMAL -> PRESENT_SRC_KHR (wgpu keeps
        // believing RENDER_ATTACHMENT, which `pre` restores next time).
        let post = self.barrier_encoder(
            ctx,
            image,
            range,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            vk::ImageLayout::PRESENT_SRC_KHR,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::AccessFlags::empty(),
        )?;
        let rendered = self.rendered[frame.image_index as usize];
        // SAFETY: semaphores are live; they are consumed by the next submission,
        // which is the one right below (nothing else submits in between).
        unsafe {
            let q = ctx.queue.as_hal::<Vulkan>().ok_or_else(|| err("not a Vulkan queue"))?;
            q.add_wait_semaphore(self.slots[frame.slot].acquired, None, vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT);
            q.add_signal_semaphore(rendered, None);
        }
        let idx = ctx.queue.submit([pre, encoder.finish(), post]);
        self.slots[frame.slot].last_submit = Some(idx);
        let swapchains = [self.swapchain];
        let indices = [frame.image_index];
        let waits = [rendered];
        let info = vk::PresentInfoKHR::default().wait_semaphores(&waits).swapchains(&swapchains).image_indices(&indices);
        // SAFETY: wgpu is not using the queue (single-threaded use after submit).
        match unsafe { self.swapchain_fn.queue_present(self.h.queue, &info) } {
            Ok(_) => Ok(()),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => unsafe { self.create_swapchain() },
            Err(e) => Err(err(format!("vkQueuePresentKHR: {e}"))),
        }
    }

    fn raw_image(&self, index: u32) -> Result<vk::Image, EngineError> {
        // SAFETY: reads the handle of a texture this presenter imported.
        unsafe {
            let t = self.textures[index as usize].0.as_hal::<Vulkan>().ok_or_else(|| err("not a Vulkan texture"))?;
            Ok(t.raw_handle())
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn barrier_encoder(
        &self,
        ctx: &GpuContext,
        image: vk::Image,
        range: vk::ImageSubresourceRange,
        old: vk::ImageLayout,
        new: vk::ImageLayout,
        src_stage: vk::PipelineStageFlags,
        src_access: vk::AccessFlags,
        dst_stage: vk::PipelineStageFlags,
        dst_access: vk::AccessFlags,
    ) -> Result<wgpu::CommandBuffer, EngineError> {
        let mut enc = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("present barrier") });
        let device = self.h.device.clone();
        // SAFETY: records one barrier into wgpu's open raw command buffer; this
        // encoder is used only through the raw API (wgpu forbids mixing).
        let ok = unsafe {
            enc.as_hal_mut::<Vulkan, _, _>(|e| {
                let Some(e) = e else { return false };
                let barrier = vk::ImageMemoryBarrier::default()
                    .old_layout(old)
                    .new_layout(new)
                    .src_access_mask(src_access)
                    .dst_access_mask(dst_access)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(range);
                device.cmd_pipeline_barrier(e.raw_handle(), src_stage, dst_stage, vk::DependencyFlags::empty(), &[], &[], &[barrier]);
                true
            })
        };
        if !ok {
            return Err(err("could not record a raw Vulkan barrier"));
        }
        Ok(enc.finish())
    }
}

/// Wraps a swapchain image (owned by the swapchain) as a wgpu render target.
fn import_image(device: &wgpu::Device, image: vk::Image, format: wgpu::TextureFormat, extent: vk::Extent2D) -> wgpu::Texture {
    let size = wgpu::Extent3d { width: extent.width, height: extent.height, depth_or_array_layers: 1 };
    let hal_desc = hal::TextureDescriptor {
        label: Some("swapchain image"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUses::COLOR_TARGET,
        memory_flags: hal::MemoryFlags::empty(),
        view_formats: vec![],
    };
    // SAFETY: the image is valid until the swapchain is destroyed, which only
    // happens after these textures are dropped and the queue is idle; the
    // no-op drop callback keeps wgpu from destroying it.
    unsafe {
        let hal_dev = device.as_hal::<Vulkan>().expect("Vulkan device");
        let hal_tex = hal_dev.texture_from_raw(image, &hal_desc, Some(Box::new(|| {})), hal::vulkan::TextureMemory::External);
        drop(hal_dev);
        device.create_texture_from_hal::<Vulkan>(
            hal_tex,
            &wgpu::TextureDescriptor {
                label: Some("swapchain image"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
            wgpu::TextureUses::UNINITIALIZED,
        )
    }
}

impl Drop for VkPresenter {
    fn drop(&mut self) {
        // SAFETY: waits for the queue, then destroys what this presenter made.
        unsafe {
            self.destroy_swapchain_objects();
            if self.swapchain != vk::SwapchainKHR::null() {
                self.swapchain_fn.destroy_swapchain(self.swapchain, None);
            }
            if self.owns_surface {
                self.surface_fn.destroy_surface(self.surface, None);
            }
        }
    }
}
