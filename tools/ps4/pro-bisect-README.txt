Frogger 2 (Rust port) - PS4 Pro package bisect
=============================================

PS4-6-frogger-enhanced.pkg (FROG20002) crashes with CE-108255-1 on your PS5, while the
native PS5 package runs. That package differs from the conservative one (FROG20000) in
three ways at once: PS4 Pro mode (param.sfo ATTRIBUTE bit 23), 7-CPU mode (bit 16) and
the Piglet "auth info" in eboot.bin (the capabilities the OpenOrbis Piglet sample uses).
Each package here changes one thing, so the crash can be pinned down. Each has its own
title ID: all of them install side by side (and beside the earlier packages).

  Letter  File                                   Title ID   ATTRIBUTE              Auth info  Framebuffers
  A       PS4-A-pro-only.pkg                     FROG20003  Pro mode (bit 23)      default    output mode (4K on a 4K TV)
  B       PS4-B-7cpu-only.pkg                    FROG20004  7-CPU mode (bit 16)    default    1920x1080
  C       PS4-C-piglet-authinfo-only.pkg         FROG20005  0                      Piglet     1920x1080
  D       PS4-D-pro-only-fb1080.pkg              FROG20006  Pro mode (bit 23)      default    forced 1920x1080
  E       PS4-E-pro-7cpu-default-authinfo.pkg    FROG20007  bits 23 + 16 + 1       default    output mode

(E is the crashing package's ATTRIBUTE with the default auth info.)

All are the game with your data bundled, CPU renderer (no Piglet is loaded by any of
them). Keep them for your own console; do not share them.

Please: install each one, start it, and report for each letter: runs / crashes (error
code) / something else (black screen, notification text). If you can, capture the kernel
log for the ones that crash (below), and also for A, so we see the Pro-mode video mode.

What this tells us:
  - A crashes, D runs     -> 4K linear framebuffers in Pro mode are the problem.
  - A and D crash         -> Pro mode itself (the ATTRIBUTE bit) is the problem.
  - B crashes             -> 7-CPU mode.
  - C crashes             -> the Piglet auth info.
  - A, B, C, D run, E crashes -> the combination (or bit 1, Cross = enter).

Kernel log (klog)
-----------------
As in the main test folder's README: send payloads/klogsrv-ps5.elf (in the
Frogger2-HW-Test folder) to the PS5's port 9021, connect to port 3232 and save what
arrives, then start the package:

    nc -q0 PS5_IP 9021 < payloads/klogsrv-ps5.elf
    nc PS5_IP 3232 | tee klog-A.txt

(or the Python one-liners from the main README). The lines start with "[frogger2]".
This build logs every video step before it is taken: "video: sceVideoOutOpen",
"video out: mode WxH ...", "video: direct memory ...; allocating ...",
"video: sceVideoOutRegisterBuffers WxH linear", "video: ready", then each step of the
first two frames ("frame: render_soft ...", "frame: flip submitted"). The last line
before a crash names the step that crashed. If VideoOut refuses 4K framebuffers the game
now falls back to 1920x1080 instead of quitting (logged).
