Frogger 2 (Rust port) - PS4/PS5 hardware test ladder
=====================================================

Built by tools/ps5/build-hw-test.sh. Six test titles, each with its own title ID, so all
of them can be installed at the same time. Try them in order: each step proves one more
piece, and where the ladder breaks tells us what is wrong.

What changed since the last attempt (CE-107694-7 on the PS5 folder, CE-108255-1 on the
PS4 package): everything Rust now uses EMULATED thread-local storage, like every native PS5
homebrew title that is known to run on hardware (the old builds used native TLS that only
emulators had accepted); the PS5 titles now carry icon0.dds/pic0.dds/pic1.dds and the
UP9000 content ID form of the proven titles; the PS4 game defaults to the CPU renderer and
the conservative package settings of the first build; log lines go to the kernel log first
thing in main.

The ladder
----------

  #  File                             Console  Title ID   What it is
  1  PS5-1-hello.pkg                  PS5      PPSA99922  C "hello": PS5_Vulkan's proven packaging,
                                                          no Rust. Screen cycles green/blue/white.
  2  PS5-2-smoke.pkg                  PS5      PPSA99921  Rust std smoke test. Green bars with a
                                                          moving white stripe = all checks passed,
                                                          red bars = a check failed (see klog).
  3  PS5-3-frogger.pkg                PS5      PPSA99920  The game (native PS5, 4K/120 Hz).
  4  PS4-4-smoke.pkg                  PS4 pkg  FROG20001  Minimal Rust PS4 title: pulsing green =
                                                          checks passed, pulsing red = a check failed.
  5  PS4-5-frogger-conservative.pkg   PS4 pkg  FROG20000  The game, ATTRIBUTE 0, default auth info,
                                                          CPU renderer (like the very first package).
  6  PS4-6-frogger-enhanced.pkg       PS4 pkg  FROG20002  The game with PS4 Pro mode + 7-CPU mode +
                                                          Piglet auth info (the settings of the
                                                          package that crashed), CPU renderer.

The PS5 .pkg files are PS5 fake packages (the new format kstuff 1.13 installs), built with
LibProsperoPkg (the same library PS5 UltraPack uses) and checked with its validator
(validation/*.validate.txt: 33 checks passed, 0 failed for each). Install them like any PS5
fPKG. The PS4 .pkg files install as before.

Before installing #5: uninstall the old "Frogger 2" PS4 package (same title ID FROG20000).

ps5-other-formats/ holds the same PS5 titles as plain folders (PPSA9992x/: copy to
/data/homebrew/ or an extended-storage drive for ShadowMountPlus, as you did before) and
the game as PPSA99920.ffpkg (UFS2 image) and PPSA99920.ffpfsc (PFS image) for
ShadowMountPlus. Use these only if the .pkg route fails, and say which one you used.

The game packages that say "-with-data" in the build contain the game data imported from
your own CD: keep them for your own console, do not share them.

Seeing the kernel log (klog) - please do this, it shows exactly how far a title got
--------------------------------------------------------------------------------

Every title writes lines to the kernel log: "[frogger2] ..." (Rust titles and the games),
"[f2hello] ..." (the C hello), "[f2launch] ..." (launch payloads). The first Rust lines are
"_start entered", "_init_env done", "constructors done, calling main", "main entered".

1. Jailbreak as usual so that elfldr is listening on port 9021 (the loader kstuff and
   ShadowMountPlus are sent to).
2. Send payloads/klogsrv-ps5.elf to the PS5's port 9021. From WSL/Linux:
       nc -q0 PS5_IP 9021 < payloads/klogsrv-ps5.elf
   or from Windows with Python installed (one line):
       python -c "import socket;s=socket.create_connection(('PS5_IP',9021));s.sendall(open('payloads/klogsrv-ps5.elf','rb').read());s.close()"
   (any payload sender tool works too). A notification "Serving /dev/klog on ...:3232"
   appears. klogsrv is ps5-payload-dev's klog server (github.com/ps5-payload-dev/klogsrv).
   If your setup already runs a klog server (e.g. one built into etaHEN), you can use that
   instead.
3. On the PC, connect to port 3232 and save what arrives:
       nc PS5_IP 3232 | tee klog.txt
   or on Windows:
       python -c "import socket;s=socket.create_connection(('PS5_IP',3232));f=open('klog.txt','wb');[f.write(b) or f.flush() for b in iter(lambda:s.recv(65536),b'')]"
   Leave it running, then start the title on the PS5. Stop with Ctrl+C.

Launching from the PC (optional): payloads/launch-<TITLEID>.elf sent to port 9021 starts
that title and puts "sceSystemServiceLaunchApp(<id>) = 0x........" in klog and a
notification (0x00000000 = started; any other value is an error code worth reporting).
The FROG2000x launchers try the PS4 titles the same way (may not be supported for PS4
titles).

Log files over FTP
------------------

The Rust titles also write frogger.log in the first writable folder of:
/data/frogger2, /user/data/frogger2, /mnt/usb0/frogger2, /mnt/usb1/frogger2,
/download0/frogger2 (app-private, not reachable over FTP), /user/temp/frogger2.
The klog line "[frogger2] log file ..." says which one was used. The C hello writes
hello.txt files and logs which paths were writable.

What to report, for each step you tried
---------------------------------------

- installed OK? icon shown?
- launch result: runs / black screen / error code (e.g. CE-107694-7, CE-108255-1,
  CE-100096-6) and when it appears (immediately, after a few seconds, ...);
- what the screen showed (colours, bars, stripe moving or frozen, game picture);
- klog.txt (or at least every line with [frogger2], [f2hello] or [f2launch], plus the
  lines just before an error), and frogger.log if one was written;
- for the PS5 titles: which format (pkg / folder / ffpkg / ffpfsc).

Quick reading: 1 fails -> the packaging/loader path on this console (not the port);
1 works but 2 fails before "main entered" -> Rust start-up/TLS; 2 works but 3 fails ->
the game itself (klog shows the last step); 4 fails -> PS4 packaging/start-up;
5 works but 6 fails -> the Pro/7-CPU/auth-info settings.
