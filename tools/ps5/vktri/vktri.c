/*
 * vktri: minimal Vulkan bring-up title for the PS5 RADV driver (PS5_Vulkan).
 *
 * Statically linked against libvulkan_radeon.ps5.a; every entry point comes
 * from vk_icdGetInstanceProcAddr (no loader). Presents a spinning triangle
 * through VK_KHR_display (RADV's VideoOut swapchain) for FRAMES frames and
 * logs each step on stdout, so a console klog or an emulator log shows exactly
 * how far it got. The same code builds for the host (-DVKTRI_HOST, against
 * the system loader) for a quick sanity run where a display is available.
 *
 * Built by tools/ps5/build-vktri.sh. SPDX-License-Identifier: GPL-3.0-or-later
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <vulkan/vulkan.h>

#include "tri_vert.h"
#include "tri_frag.h"

#define FRAMES 600
/* KytyPS5 aborts on fflush (its libc HLE handles only printf to stdout); VKTRI_FLUSH=1 for a console. */
#ifdef VKTRI_FLUSH
#define FLUSH() fflush(stdout)
#else
#define FLUSH() ((void)0)
#endif

VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL vk_icdGetInstanceProcAddr(VkInstance instance, const char *name);

static void step(const char *what, VkResult r)
{
   printf("[vktri] %s: %d\n", what, (int)r);
   FLUSH();
   if (r < 0) {
      printf("[vktri] FAILED at %s\n", what);
      FLUSH();
      exit(1);
   }
}

#define LOAD(inst, name) PFN_vk##name vk##name = (PFN_vk##name)vk_icdGetInstanceProcAddr(inst, "vk" #name)

int main(void)
{
   /* KytyPS5's libc HLE aborts on any write to a stream other than stdout;
    * Mesa reports on stderr. On the console this is harmless. */
   stderr = stdout;
   setenv("MESA_VK_IGNORE_CONFORMANCE_WARNING", "1", 1);
   printf("[vktri] start\n");
   FLUSH();

   LOAD(VK_NULL_HANDLE, CreateInstance);
   step("vk_icdGetInstanceProcAddr(vkCreateInstance)", vkCreateInstance ? VK_SUCCESS : VK_ERROR_INITIALIZATION_FAILED);
   const char *inst_ext[] = {VK_KHR_SURFACE_EXTENSION_NAME, VK_KHR_DISPLAY_EXTENSION_NAME};
   VkApplicationInfo app = {.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO, .pApplicationName = "vktri", .apiVersion = VK_API_VERSION_1_1};
   VkInstanceCreateInfo ici = {.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, .pApplicationInfo = &app,
                               .enabledExtensionCount = 2, .ppEnabledExtensionNames = inst_ext};
   VkInstance inst;
   step("vkCreateInstance", vkCreateInstance(&ici, NULL, &inst));

   LOAD(inst, EnumeratePhysicalDevices);
   LOAD(inst, GetPhysicalDeviceProperties);
   LOAD(inst, GetPhysicalDeviceQueueFamilyProperties);
   LOAD(inst, CreateDevice);
   LOAD(inst, GetDeviceQueue);
   LOAD(inst, GetPhysicalDeviceDisplayPropertiesKHR);
   LOAD(inst, GetDisplayModePropertiesKHR);
   LOAD(inst, CreateDisplayPlaneSurfaceKHR);
   LOAD(inst, GetPhysicalDeviceSurfaceSupportKHR);
   LOAD(inst, GetPhysicalDeviceSurfaceCapabilitiesKHR);
   LOAD(inst, GetPhysicalDeviceSurfaceFormatsKHR);

   uint32_t n = 1;
   VkPhysicalDevice pd;
   VkResult r = vkEnumeratePhysicalDevices(inst, &n, &pd);
   step("vkEnumeratePhysicalDevices", n == 1 ? (r == VK_INCOMPLETE ? VK_SUCCESS : r) : VK_ERROR_INITIALIZATION_FAILED);
   VkPhysicalDeviceProperties props;
   vkGetPhysicalDeviceProperties(pd, &props);
   printf("[vktri] device: %s, Vulkan %u.%u\n", props.deviceName, VK_API_VERSION_MAJOR(props.apiVersion), VK_API_VERSION_MINOR(props.apiVersion));

   VkQueueFamilyProperties fam[8];
   n = 8;
   vkGetPhysicalDeviceQueueFamilyProperties(pd, &n, fam);
   uint32_t family = 0;
   for (uint32_t i = 0; i < n; i++)
      if (fam[i].queueFlags & VK_QUEUE_GRAPHICS_BIT) {
         family = i;
         break;
      }
   float prio = 1.0f;
   VkDeviceQueueCreateInfo qci = {.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO, .queueFamilyIndex = family, .queueCount = 1, .pQueuePriorities = &prio};
   const char *dev_ext[] = {VK_KHR_SWAPCHAIN_EXTENSION_NAME};
   VkDeviceCreateInfo dci = {.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO, .queueCreateInfoCount = 1, .pQueueCreateInfos = &qci,
                             .enabledExtensionCount = 1, .ppEnabledExtensionNames = dev_ext};
   VkDevice dev;
   step("vkCreateDevice", vkCreateDevice(pd, &dci, NULL, &dev));
   VkQueue queue;
   vkGetDeviceQueue(dev, family, 0, &queue);

#define DLOAD(name) PFN_vk##name vk##name = (PFN_vk##name)vk_icdGetInstanceProcAddr(inst, "vk" #name)
   DLOAD(CreateSwapchainKHR);
   DLOAD(GetSwapchainImagesKHR);
   DLOAD(AcquireNextImageKHR);
   DLOAD(QueuePresentKHR);
   DLOAD(CreateImageView);
   DLOAD(CreateRenderPass);
   DLOAD(CreateFramebuffer);
   DLOAD(CreateShaderModule);
   DLOAD(CreatePipelineLayout);
   DLOAD(CreateGraphicsPipelines);
   DLOAD(CreateCommandPool);
   DLOAD(AllocateCommandBuffers);
   DLOAD(BeginCommandBuffer);
   DLOAD(EndCommandBuffer);
   DLOAD(CmdBeginRenderPass);
   DLOAD(CmdEndRenderPass);
   DLOAD(CmdBindPipeline);
   DLOAD(CmdSetViewport);
   DLOAD(CmdSetScissor);
   DLOAD(CmdPushConstants);
   DLOAD(CmdDraw);
   DLOAD(CreateSemaphore);
   DLOAD(CreateFence);
   DLOAD(WaitForFences);
   DLOAD(ResetFences);
   DLOAD(QueueSubmit);
   DLOAD(DeviceWaitIdle);

   VkDisplayPropertiesKHR disp;
   n = 1;
   r = vkGetPhysicalDeviceDisplayPropertiesKHR(pd, &n, &disp);
   step("vkGetPhysicalDeviceDisplayPropertiesKHR", n ? (r == VK_INCOMPLETE ? VK_SUCCESS : r) : VK_ERROR_INITIALIZATION_FAILED);
   printf("[vktri] display: %s %ux%u\n", disp.displayName ? disp.displayName : "?", disp.physicalResolution.width, disp.physicalResolution.height);
   VkDisplayModePropertiesKHR mode;
   n = 1;
   r = vkGetDisplayModePropertiesKHR(pd, disp.display, &n, &mode);
   step("vkGetDisplayModePropertiesKHR", n ? (r == VK_INCOMPLETE ? VK_SUCCESS : r) : VK_ERROR_INITIALIZATION_FAILED);
   VkExtent2D ext = mode.parameters.visibleRegion;
   printf("[vktri] mode: %ux%u @ %.3f Hz\n", ext.width, ext.height, mode.parameters.refreshRate / 1000.0);
   VkDisplaySurfaceCreateInfoKHR sci = {.sType = VK_STRUCTURE_TYPE_DISPLAY_SURFACE_CREATE_INFO_KHR, .displayMode = mode.displayMode,
                                        .transform = VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR, .globalAlpha = 1.0f,
                                        .alphaMode = VK_DISPLAY_PLANE_ALPHA_OPAQUE_BIT_KHR, .imageExtent = ext};
   VkSurfaceKHR surface;
   step("vkCreateDisplayPlaneSurfaceKHR", vkCreateDisplayPlaneSurfaceKHR(inst, &sci, NULL, &surface));
   VkBool32 supported = VK_FALSE;
   vkGetPhysicalDeviceSurfaceSupportKHR(pd, family, surface, &supported);
   step("surface support", supported ? VK_SUCCESS : VK_ERROR_SURFACE_LOST_KHR);
   VkSurfaceCapabilitiesKHR caps;
   step("vkGetPhysicalDeviceSurfaceCapabilitiesKHR", vkGetPhysicalDeviceSurfaceCapabilitiesKHR(pd, surface, &caps));
   VkSurfaceFormatKHR fmts[16];
   n = 16;
   step("vkGetPhysicalDeviceSurfaceFormatsKHR", vkGetPhysicalDeviceSurfaceFormatsKHR(pd, surface, &n, fmts));
   VkSurfaceFormatKHR fmt = fmts[0];
   for (uint32_t i = 0; i < n; i++)
      if (fmts[i].format == VK_FORMAT_B8G8R8A8_UNORM || fmts[i].format == VK_FORMAT_R8G8B8A8_UNORM) {
         fmt = fmts[i];
         break;
      }
   printf("[vktri] surface: %u formats, using %d; images %u..%u\n", n, (int)fmt.format, caps.minImageCount, caps.maxImageCount);
   if (caps.currentExtent.width != UINT32_MAX)
      ext = caps.currentExtent;

   uint32_t want = caps.minImageCount < 3 ? 3 : caps.minImageCount;
   if (caps.maxImageCount && want > caps.maxImageCount)
      want = caps.maxImageCount;
   VkSwapchainCreateInfoKHR swci = {.sType = VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR, .surface = surface, .minImageCount = want,
                                    .imageFormat = fmt.format, .imageColorSpace = fmt.colorSpace, .imageExtent = ext,
                                    .imageArrayLayers = 1, .imageUsage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT,
                                    .preTransform = caps.currentTransform, .compositeAlpha = VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR,
                                    .presentMode = VK_PRESENT_MODE_FIFO_KHR, .clipped = VK_TRUE};
   VkSwapchainKHR sc;
   step("vkCreateSwapchainKHR", vkCreateSwapchainKHR(dev, &swci, NULL, &sc));
   VkImage images[8];
   uint32_t count = 8;
   step("vkGetSwapchainImagesKHR", vkGetSwapchainImagesKHR(dev, sc, &count, images));

   VkAttachmentDescription att = {.format = fmt.format, .samples = VK_SAMPLE_COUNT_1_BIT, .loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR,
                                  .storeOp = VK_ATTACHMENT_STORE_OP_STORE, .stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE,
                                  .stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE, .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED,
                                  .finalLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR};
   VkAttachmentReference ref = {0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
   VkSubpassDescription sub = {.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS, .colorAttachmentCount = 1, .pColorAttachments = &ref};
   VkSubpassDependency depy = {.srcSubpass = VK_SUBPASS_EXTERNAL, .dstSubpass = 0,
                               .srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
                               .dstStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
                               .dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT};
   VkRenderPassCreateInfo rpci = {.sType = VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO, .attachmentCount = 1, .pAttachments = &att,
                                  .subpassCount = 1, .pSubpasses = &sub, .dependencyCount = 1, .pDependencies = &depy};
   VkRenderPass rp;
   step("vkCreateRenderPass", vkCreateRenderPass(dev, &rpci, NULL, &rp));
   VkFramebuffer fbs[8];
   for (uint32_t i = 0; i < count; i++) {
      VkImageViewCreateInfo vci = {.sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO, .image = images[i], .viewType = VK_IMAGE_VIEW_TYPE_2D,
                                   .format = fmt.format, .subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}};
      VkImageView view;
      step("vkCreateImageView", vkCreateImageView(dev, &vci, NULL, &view));
      VkFramebufferCreateInfo fci = {.sType = VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO, .renderPass = rp, .attachmentCount = 1,
                                     .pAttachments = &view, .width = ext.width, .height = ext.height, .layers = 1};
      step("vkCreateFramebuffer", vkCreateFramebuffer(dev, &fci, NULL, &fbs[i]));
   }

   VkShaderModuleCreateInfo vsci = {.sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO, .codeSize = sizeof(tri_vert), .pCode = tri_vert};
   VkShaderModuleCreateInfo fsci = {.sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO, .codeSize = sizeof(tri_frag), .pCode = tri_frag};
   VkShaderModule vs, fs;
   step("vkCreateShaderModule(vert)", vkCreateShaderModule(dev, &vsci, NULL, &vs));
   step("vkCreateShaderModule(frag)", vkCreateShaderModule(dev, &fsci, NULL, &fs));
   VkPushConstantRange pcr = {VK_SHADER_STAGE_VERTEX_BIT, 0, sizeof(float)};
   VkPipelineLayoutCreateInfo plci = {.sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO, .pushConstantRangeCount = 1, .pPushConstantRanges = &pcr};
   VkPipelineLayout layout;
   step("vkCreatePipelineLayout", vkCreatePipelineLayout(dev, &plci, NULL, &layout));
   VkPipelineShaderStageCreateInfo stages[2] = {
      {.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, .stage = VK_SHADER_STAGE_VERTEX_BIT, .module = vs, .pName = "main"},
      {.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, .stage = VK_SHADER_STAGE_FRAGMENT_BIT, .module = fs, .pName = "main"}};
   VkPipelineVertexInputStateCreateInfo vin = {.sType = VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
   VkPipelineInputAssemblyStateCreateInfo ia = {.sType = VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,
                                                .topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST};
   VkPipelineViewportStateCreateInfo vp = {.sType = VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO, .viewportCount = 1, .scissorCount = 1};
   VkPipelineRasterizationStateCreateInfo rs = {.sType = VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
                                                .polygonMode = VK_POLYGON_MODE_FILL, .cullMode = VK_CULL_MODE_NONE, .lineWidth = 1.0f};
   VkPipelineMultisampleStateCreateInfo ms = {.sType = VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
                                              .rasterizationSamples = VK_SAMPLE_COUNT_1_BIT};
   VkPipelineColorBlendAttachmentState cba = {.colorWriteMask = 0xf};
   VkPipelineColorBlendStateCreateInfo cb = {.sType = VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO, .attachmentCount = 1, .pAttachments = &cba};
   VkDynamicState dyn[2] = {VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR};
   VkPipelineDynamicStateCreateInfo ds = {.sType = VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO, .dynamicStateCount = 2, .pDynamicStates = dyn};
   VkGraphicsPipelineCreateInfo gpci = {.sType = VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO, .stageCount = 2, .pStages = stages,
                                        .pVertexInputState = &vin, .pInputAssemblyState = &ia, .pViewportState = &vp,
                                        .pRasterizationState = &rs, .pMultisampleState = &ms, .pColorBlendState = &cb,
                                        .pDynamicState = &ds, .layout = layout, .renderPass = rp};
   VkPipeline pipe;
   step("vkCreateGraphicsPipelines", vkCreateGraphicsPipelines(dev, VK_NULL_HANDLE, 1, &gpci, NULL, &pipe));

   VkCommandPoolCreateInfo cpci = {.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
                                   .flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT, .queueFamilyIndex = family};
   VkCommandPool pool;
   step("vkCreateCommandPool", vkCreateCommandPool(dev, &cpci, NULL, &pool));
   VkCommandBufferAllocateInfo cbai = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO, .commandPool = pool,
                                       .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY, .commandBufferCount = 1};
   VkCommandBuffer cmd;
   step("vkAllocateCommandBuffers", vkAllocateCommandBuffers(dev, &cbai, &cmd));
   VkSemaphoreCreateInfo semci = {.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
   VkSemaphore acquired, rendered[8];
   step("vkCreateSemaphore", vkCreateSemaphore(dev, &semci, NULL, &acquired));
   for (uint32_t i = 0; i < count; i++)
      step("vkCreateSemaphore", vkCreateSemaphore(dev, &semci, NULL, &rendered[i]));
   VkFenceCreateInfo fci = {.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
   VkFence fence;
   step("vkCreateFence", vkCreateFence(dev, &fci, NULL, &fence));

   for (int f = 0; f < FRAMES; f++) {
      uint32_t idx;
      r = vkAcquireNextImageKHR(dev, sc, UINT64_MAX, acquired, VK_NULL_HANDLE, &idx);
      if (f == 0)
         step("first vkAcquireNextImageKHR", r);
      VkCommandBufferBeginInfo bi = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
                                     .flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT};
      vkBeginCommandBuffer(cmd, &bi);
      float t = f / 60.0f;
      VkClearValue clear = {.color = {{0.1f, 0.1f + 0.1f * sinf(t), 0.25f, 1.0f}}};
      VkRenderPassBeginInfo rbi = {.sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO, .renderPass = rp, .framebuffer = fbs[idx],
                                   .renderArea = {{0, 0}, ext}, .clearValueCount = 1, .pClearValues = &clear};
      vkCmdBeginRenderPass(cmd, &rbi, VK_SUBPASS_CONTENTS_INLINE);
      vkCmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipe);
      VkViewport v = {0, 0, (float)ext.width, (float)ext.height, 0, 1};
      VkRect2D s = {{0, 0}, ext};
      vkCmdSetViewport(cmd, 0, 1, &v);
      vkCmdSetScissor(cmd, 0, 1, &s);
      vkCmdPushConstants(cmd, layout, VK_SHADER_STAGE_VERTEX_BIT, 0, sizeof(float), &t);
      vkCmdDraw(cmd, 3, 1, 0, 0);
      vkCmdEndRenderPass(cmd);
      vkEndCommandBuffer(cmd);
      VkPipelineStageFlags wait = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
      VkSubmitInfo si = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO, .waitSemaphoreCount = 1, .pWaitSemaphores = &acquired,
                         .pWaitDstStageMask = &wait, .commandBufferCount = 1, .pCommandBuffers = &cmd,
                         .signalSemaphoreCount = 1, .pSignalSemaphores = &rendered[idx]};
      r = vkQueueSubmit(queue, 1, &si, fence);
      if (f == 0)
         step("first vkQueueSubmit", r);
      VkPresentInfoKHR pi = {.sType = VK_STRUCTURE_TYPE_PRESENT_INFO_KHR, .waitSemaphoreCount = 1, .pWaitSemaphores = &rendered[idx],
                             .swapchainCount = 1, .pSwapchains = &sc, .pImageIndices = &idx};
      r = vkQueuePresentKHR(queue, &pi);
      if (f == 0)
         step("first vkQueuePresentKHR", r);
      r = vkWaitForFences(dev, 1, &fence, VK_TRUE, UINT64_MAX);
      if (f == 0)
         step("first frame fence", r);
      vkResetFences(dev, 1, &fence);
      if (f % 60 == 59) {
         printf("[vktri] %d frames presented\n", f + 1);
         FLUSH();
      }
   }
   vkDeviceWaitIdle(dev);
   printf("[vktri] done: %d frames\n", FRAMES);
   FLUSH();
   return 0;
}
