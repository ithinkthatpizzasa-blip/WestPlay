# WestPlay on macOS

Porting WestPlay to macOS: what the Mac game is, what was checked on a real Mac, and how each part of WestPlay
gets there. Every fact below was checked on the machine named, unless it says otherwise.

## Phase 0: the Mac game (2026-11-01)

Checked on an Apple M1 Max MacBook Pro, 32 GB, macOS 27.0.1 (26A434), ETS2 from Steam.

### Where it is

| What | Where |
|---|---|
| Steam's libraries | `~/Library/Application Support/Steam/steamapps/libraryfolders.vdf` (same VDF format as Windows'; here a second library on `/Volumes/Extreme SSD/SteamLibrary`, without ETS2) |
| The game | `~/Library/Application Support/Steam/steamapps/common/Euro Truck Simulator 2/` |
| Its bundle | `…/Euro Truck Simulator 2.app` (`CFBundleIdentifier` `com.scssoft.ets2`) |
| The executable | `…/Euro Truck Simulator 2.app/Contents/MacOS/eurotrucks2` |
| The `.scs` archives | The game's folder itself, beside the `.app` (`base.scs`, `def.scs`, `dlc_*.scs`…), as on Windows beside `bin/` |
| Bundled libraries | `…/Contents/Frameworks/`: `libMoltenVK.dylib`, `libfmod.dylib`, `libfmodstudio.dylib`, `libsteam_api.dylib` |
| SDK plugins | `…/Euro Truck Simulator 2.app/Contents/MacOS/plugins/*.dylib` (proved below) |
| Home folder (the Windows `Documents\Euro Truck Simulator 2`) | `~/Library/Application Support/Euro Truck Simulator 2/` (the game's log: `[ufs] Home directory: '…'`) |
| `config.cfg`, `game.log.txt` | In the home folder |
| Profiles | `steam_profiles/<hex name>/` (Steam Cloud) and `profiles/` in the home folder; a profile's controls file is `controls_osx.sii` (Windows: `controls.sii`) |
| Mods | `mod/` in the home folder; Workshop mods in `~/Library/Application Support/Steam/steamapps/workshop/content/227300/` |

### The game's version: 1.57, not 1.61

The Mac game is **1.57.2.7** (`[ufs] Loaded pack set version 1.57.2.7`; the plugin is told
`Euro Truck Simulator 2 1.57.2.7s`, telemetry game version 1.18, API 1.1). Its signature is dated
2025-12-11 and Steam has nothing newer for it (`appmanifest_227300.acf`, build 21149108), while the Windows game,
which WestPlay is made for, is at 1.61. Whether SCS has stopped updating the Mac build, or only paused, isn't
stated anywhere found (a web search found no SCS statement either way); the Steam store still lists macOS.

This matters wherever WestPlay depends on the game's own code or data rather than the SDK:
- The plugin's `navigation.rs` (changing the GPS route) and `camera.rs` (the camera's position) find the game's
  code by its bytes in the **Windows** executable (MSVC's code, 1.61). The Mac executable is another compiler's
  (Apple clang) code of another version: every pattern has to be found again in it, and the navigator's offsets
  checked for 1.57.
- The host's `route.rs` reads the game's memory from outside (the navigator's route). See "Reading the game's
  memory" below.
- The screen mod (`crates/setup/src/game_mod.rs`) patches the game's GPS UI scripts of 1.61, and the trucks'
  screens WestPlay supports include 1.61's: each needs checking against 1.57's files (for example whether the
  Volvo FH 2024 and Renault T 2024's interiors are the same).

### Architecture: x86_64 only, under Rosetta 2

`lipo -info` / `file`: `eurotrucks2` is x86_64 only (a fat file with one architecture). On this arm64 Mac it runs
under Rosetta 2: the game logs `CPU0: GenuineIntel [VirtualApple @ 2.50GHz]`, and the probe plugin reads
`sysctl.proc_translated = 1`. The bundled libraries are universal (x86_64 + arm64), but a process has one
architecture, so **WestPlay's plugin must be x86_64** (`x86_64-apple-darwin`); an arm64 or arm64-only plugin
can't load into it. A universal plugin would only matter if SCS ships an arm64 game.

Consequences:
- x86_64 code hooking works as on Windows: the plugin is x86_64 code patching x86_64 code, and Rosetta handles
  code that changes (it re-translates). MinHook itself is Windows-only (it calls `VirtualAlloc`/`VirtualProtect`),
  so the Mac plugin needs another hooker, but the technique (inline x86_64 detours) is the same.
- The **host** (the companion's browser, WestPlay's own process) is native arm64; the bridge between them is
  shared memory, which works across a translated and a native process (same page layout, the `#[repr(C)]` structs
  have the same layout on both: little-endian, same alignment). To be confirmed by a test in Phase 1.

### The graphics API: OpenGL 4.1 (over Metal), with Vulkan built in too

`otool -L eurotrucks2` links OpenGL, Metal, IOSurface, QuartzCore and `@executable_path/../Frameworks/libMoltenVK.dylib`.
The game has two rendering devices:
- **OpenGL** (`gl`): what this game uses. `game.log.txt`: `[gfx] Selected rendering device: gl`,
  `[gl] version: 4.1 Metal - 91.7`, `[gl] driver: Apple M1 Max`, `[gfx] Using the COCOA display backend`,
  `[disp] Adapter 0: CGL on COCOA backend`. `config.cfg` has `uset r_device ""` (the default) and
  `r_macgl_*` settings. Apple's OpenGL runs on Metal underneath (`AppleMetalOpenGLRenderer.bundle` is loaded).
- **Vulkan through MoltenVK**: the executable imports `vkCreateInstance`/`vkGetInstanceProcAddr` and logs `[vk] …`
  lines, and has a `ns_vulkan_view_t`. The device can be chosen on the command line (`-rdevice` / `-r_device`,
  `[gfx] Render device override: %s`). Its name is **`vk`**: a launch with `-rdevice vk` (2026-11-01) logged
  `[gfx] Render device override: vk`, `[vk] 0: Apple M1 Max 0.0 (integrated) (1.2.296)`, `VK_EXT_metal_surface`.
  It runs, but **drew the menu magnified, only its top-left part on the screen** (fullscreen at 3456 × 2234 on
  the Retina display), so it's not something players would use as it is; the setting isn't kept (`config.cfg`
  still has `r_device ""` after it). Vulkan support is left out unless that changes.

How the game reaches OpenGL: it **resolves GL functions at run time** (`dlopen` of
`/System/Library/Frameworks/OpenGL.framework/Versions/Current/Libraries/libGL.dylib` and `dlsym`, the names
`glTexImage2D`, `glTexStorage2D`, `glBindTexture`… as strings in the executable), importing only a few directly
(`_glFlush`, `_glFinish`, `_glSwapAPPLE`, `CGL*`, `NSOpenGLContext`). So rebinding its imports (fishhook) won't
catch most GL calls; the plugin has to detour the functions themselves in `libGL.dylib` (inline hooks, x86_64), or
catch `dlsym` before the game resolves them (too late: the plugin loads after the renderer is up). Phase 3 starts
from the detour of `libGL`'s own functions (`glTexStorage2D`/`glTexImage2D` to see the screen textures created at
1280 × (720 + layout), `glBindTexture` to see them bound, and the end of a frame from `CGLFlushDrawable` or the
`NSOpenGLContext` flush), the same scheme as `d3d.rs` on Windows.

When the plugin's `scs_telemetry_init` runs, it's on the game's main thread with a CGL context current
(`0x7fe936891800` in one run): the GL context exists before the plugin loads.

### Where SDK plugins load from: proved

A minimal plugin, `tools/macos_probe/probe.cpp` (x86_64, exports `scs_telemetry_init`/`scs_telemetry_shutdown`,
logs through the SDK's log callback; built by `tools/macos_probe/build.sh`), copied to
`Euro Truck Simulator 2.app/Contents/MacOS/plugins/westplay_probe.dylib`. `game.log.txt`:

```
loading 'westplay_probe' '…/Euro Truck Simulator 2.app/Contents/MacOS/plugins/westplay_probe.dylib'
[westplay-probe] loaded: Euro Truck Simulator 2 1.57.2.7s (eut2), game version 1.18, api 1.1
[westplay-probe] init on thread 2432807 (main: 1), current CGL context 0x7fe936891800
[westplay-probe] pid 84769, plugin arch x86_64, Rosetta translated: 1
…
[westplay-probe] shutdown after 0 frames
unloaded 'westplay_probe'
```

Plugins load about 36 s into the start, at the main menu, as on Windows; no frame events came in the menu (they
start with a profile loaded). Another player-installed plugin (`adblue_airpressure.dylib`, ad-hoc signed, x86_64)
loads from the same folder.

### Code signing, library validation, quarantine

`codesign -dvvv` on the app: signed by "Developer ID Application: SCS Software s.r.o. (8XZ4MF5UT3)", notarized
(ticket stapled), **hardened runtime on** (`flags=0x10000(runtime)`). Its entitlements:

| Entitlement | Value | Meaning for WestPlay |
|---|---|---|
| `com.apple.security.cs.disable-library-validation` | true | It loads libraries not signed by SCS: an ad-hoc-signed plugin loads (proved) |
| `com.apple.security.cs.allow-dyld-environment-variables` | true | `DYLD_INSERT_LIBRARIES` would work too (not needed) |
| `com.apple.security.get-task-allow` | true | The process may be debugged / have its task port taken (see below) |
| `com.apple.security.device.audio-input` | true | (Microphone; its usage string is missing, the game logs that) |

- An **ad-hoc-signed** (`codesign -s -`) dylib loads (proved). An unsigned one wasn't tried: arm64 code must be
  signed, x86_64 code needn't be, but setup will ad-hoc sign it anyway.
- **Quarantine doesn't stop it**: a copy with `com.apple.quarantine` set (as a browser download has) loaded the
  same (`westplay_probe_quarantined`). Gatekeeper checks quarantined apps when they're opened, not libraries a
  running app `dlopen`s. Setup still removes the attribute, so nothing depends on that.
- **Writing into the bundle breaks its seal**: `codesign --verify` on the app says "a sealed resource is missing or
  invalid" once anything is in `Contents/MacOS/plugins/` (the folder the game itself reads plugins from, so there's
  no other place). The game started regardless (it had been launched before; Gatekeeper's first-launch check
  is already done). Steam's "Verify integrity of game files" doesn't remove extra files. To keep an eye on: a
  fresh install's first launch with WestPlay already in place. Uninstall leaves the folder as it was.

### Reading the game's memory (the host's `route.rs`)

On Windows the host reads the game's memory from outside with `ReadProcessMemory`. On macOS that's
`task_for_pid` + `mach_vm_read`, which a normal app is refused even for a process with `get-task-allow` unless it
has the `com.apple.security.cs.debugger` entitlement and the user allows it (Developer Tools), or runs as root.
Plan: move that reading into the plugin, which is inside the game, and pass the result through the bridge, rather
than asking players for debugger rights. (To be confirmed in Phase 3 when `route.rs` is read closely.)

### Input

The game reads the keyboard and mouse through Cocoa events and HID (`[hid] Detected HID device …` for the
keyboard, mouse and trackpad). The other plugin here logs "hotkeys ready (Input Monitoring allowed)": it uses
the Input Monitoring permission (IOHID or an event tap). WestPlay's plan stays the one in the brief: swizzle
`-[NSApplication sendEvent:]` / the game window's `sendEvent:` inside the game, needing no permission; checked in
Phase 3 whether the game's mouse look comes through Cocoa events or HID (if HID, held-back Cocoa events won't stop
the camera turning).

### TruckersMP: doesn't apply

TruckersMP has no macOS client: it runs only with the Windows game (also under Wine/Proton on Linux). The only
Mac route found is an unofficial launcher whose own page says it's not maintained and not affiliated with
TruckersMP. So `crates/plugin/src/truckersmp.rs`, `crates/setup/src/truckersmp.rs` and the map's other players
have nothing to work with on a Mac game. On macOS setup never finds a TruckersMP installation (Phase 1), the
companion won't show TruckersMP there.

Sources: [unofficial TruckersMP macOS launcher](https://github.com/matyash12/unofficial-truckersmp-macos-launcher),
[truckersmp-cli](https://github.com/truckersmp-cli/truckersmp-cli/blob/main/README.md),
[Konvoi's supported games](https://docs.konvoi.gg/reference/supported-games).

### Toolchain on this Mac

- Xcode at `/Applications/Xcode.app` (Apple clang 21.0.0).
- Rust 1.99.1 stable (rustup, installed for this port), targets `aarch64-apple-darwin`, `x86_64-apple-darwin`,
  `x86_64-pc-windows-msvc` (the last for `cargo check` of the Windows build; it can't link here).
- Node.js 24.22.2 in `~/.local/node-v24` (Node 20 stays the default on this Mac; put
  `~/.local/node-v24/bin` first on `PATH` for WestPlay).

### What this means for the phases after

1. **Plugin target**: `x86_64-apple-darwin` only, for now.
2. **Screen takeover** is OpenGL: detours in `libGL.dylib`, textures recognised by size as on Windows, WestPlay's
   frame uploaded into a GL texture of the plugin's own. Vulkan support only if the owner's test shows the Vulkan
   device works well enough that players use it.
3. **The game's code** (navigation, camera) has to be found again in the 1.57 Mac executable, and the game's data
   (GPS scripts, trucks) checked against 1.57.
4. **TruckersMP** never found on macOS (setup has no TruckersMP folder there; the plugin's part in Phase 3).
5. **The route** read inside the game by the plugin, not from outside.

### Still open from Phase 0

- (Done: the Vulkan device is `vk`; it runs but draws magnified on the Retina display.)
- Which thread draws, and whether the CGL context on the SDK's frame events is the drawing one (frame events
  need a profile loaded and driving: the owner, or the next in-game session).
- `sdk reload` / `sdk unload` in the console (Phase 3, with the real plugin).

## Phase 1: portable foundations (2026-11-01)

Done, and run on this Mac. `cargo test -p westplay-shared -p westplay-mapdata -p westplay-setup`: 30 + 163 + 71
tests pass (8 more, which look at this Mac's own game, run by hand: below).

### The bridge (`crates/shared/src/platform`)

- `platform::Region`: named shared memory, Windows' `CreateFileMappingW` (moved, unchanged) or macOS's
  `shm_open` + `ftruncate` + `mmap`. `BridgeMapping::open/bridge/matches` are as they were; `open`'s error is the
  OS's (`windows::core::Error` on Windows, as before; `io::Error` on macOS).
- Names: `Local\WestPlay.Bridge.v24` on Windows (unchanged), `/WestPlay.Bridge.v24` on macOS. macOS takes at
  most 31 bytes (`PSHMNAMLEN`); tests check WestPlay's names leave room for a version up to six digits longer.
- **Fresh when no one holds it**, as on Windows: macOS keeps POSIX shared memory until it's unlinked (through
  crashes), where Windows frees a mapping with its last handle. Each holder keeps a shared `flock` on
  `<user temp>/WestPlay.Bridge.v24.holders` while mapped (dropped with the process however it ends); an opener
  that can lock it exclusively is alone and unlinks the old object first. A second lock (`….open`), held only
  while opening, keeps two openers from both thinking they're alone. The lock folder is
  `confstr(_CS_DARWIN_USER_TEMP_DIR)`, the same for the game and the app whatever their environments.
- Checked across architectures: an x86_64 process under Rosetta (as the game is) and a native arm64 one (the
  app) see each other's writes, `SeqLock` strings included, and once both are gone the next opener gets a new,
  zeroed bridge.

### Frames on macOS (`crates/shared/src/frames.rs`)

A block of its own, `/WestPlay.Frames.v24`: three slots of 1280 × 720 BGRA (3.5 MiB each), a slot index for
the newest (with a "new" bit) and which slot each side holds. The app writes into its slot and swaps it in as the
newest; the plugin swaps the newest for the one it's done with. Neither waits, ever; a frame the plugin didn't
take is written over. A side that starts again takes the slot neither of the others hold. Tests: newest frame
once, slots always shared out, restart, a writer thread and a reader never seeing a torn frame (2000 frames).
Upgrading to a cross-process IOSurface is left for Phase 3, if the copy shows up in the frame time.

### Map data

`cargo test -p westplay-mapdata`: 163 pass, unchanged. Read from the Mac game's own 1.57 archives (110 mounted
in 0.16 s), the map around Berlin (15 km) exported in 11 s: 629 000 items, 0 missing prefabs, 0 unrouted roads.

### Setup (`crates/setup/src/platform`)

The OS's parts moved behind `platform` (Windows' moved unchanged; `pick.rs` to `platform/windows/pick.rs`):

| | Windows | macOS |
|---|---|---|
| "Documents" (the game's folder's home; WestPlay's own folder) | Known Folder | `~/Library/Application Support` (not `~/Documents`: macOS asks each program's leave for that, the game's too) |
| Steam | registry | `~/Library/Application Support/Steam` |
| Other installs | registry's installed programs | none (the Mac game is Steam's; another folder can be chosen) |
| Executable | `bin/win_x64/eurotrucks2.exe` | `Euro Truck Simulator 2.app/Contents/MacOS/eurotrucks2` |
| Version | the executable's version resource | `version.scs`'s `version.sii` (`version: "1.57.2.7"`); the `.app`'s own version (1.22.2) isn't the game's |
| Game running | ToolHelp snapshot | `proc_listallpids` + `proc_pidpath` |
| Plugin | `bin/win_x64/plugins/westplay.dll` | `…/Contents/MacOS/plugins/westplay.dylib`, quarantine removed, ad-hoc signed if it isn't signed |
| Uninstall | removes the file | removes the file, and the `plugins` folder if that leaves it empty (the `.app` as it was) |
| Administrator rights | UAC (`runas`) | macOS's password prompt (`osascript … with administrator privileges`); only for a library an administrator owns |
| Pickers | `IFileOpenDialog` | `NSOpenPanel` (`rfd`, shown on the main thread) |
| TruckersMP | `%APPDATA%\TruckersMP` | none |

Checked on this Mac (the ignored tests, run by hand):
- The game found: Steam's, version 1.57.2.7, 102 DLCs, its Workshop folder.
- Profiles: three, in Steam Cloud, the selected one ("Nirav Karnik", from `game.log.txt`), their mods (ProMods
  2.77 and Workshop mods), which matter to the map.
- Installing a real x86_64 dylib, unsigned and in quarantine: it lands signed and out of quarantine, and
  uninstalling leaves no `plugins` folder (another plugin there keeps it).
- **The plugin must be shipped signed**: setup tells the installed plugin current by its bytes, and signing it on
  the way in changes them (seen as "another version"). The build signs it (Phase 3/4); then the copy is the same.
- WestPlay's screen mod built from this game: patched the stock 7" screen (`generic`), DAF 2021, both Scania
  2025, Iveco S-Way, Volvo FH, the tablet and the toy GPS (`setra-516`). Not found: `renault-t-2024` (the
  Renault T 2024 isn't in 1.57: the README's list of trucks differs on the Mac), and Oyuncuyusbis' coaches
  (mods not installed here).

### Windows

`cargo check --workspace --exclude westplay-app --exclude westplay-plugin --all-targets --target
x86_64-pc-windows-msvc` passes (shared, mapdata, setup, host, their tests included), and clippy shows nothing in
the files changed.

All of it, the plugin and the app too, with `cargo-xwin` (Microsoft's C runtime and Windows SDK headers, fetched by it, its licence
accepted by the owner, 2026-11-01), compiling MinHook's C with Apple's own `clang`, and `llvm-lib` taken from
Rust's `llvm-tools` (its `llvm-ar`, which works as `llvm-lib` when named so):

```sh
rustup component add llvm-tools
mkdir -p ~/.local/westplay-xwin-bin
ln -sf "$(find ~/.rustup/toolchains/stable-aarch64-apple-darwin -name llvm-ar -type f)" ~/.local/westplay-xwin-bin/llvm-lib
PATH=~/.local/westplay-xwin-bin:$PATH cargo xwin check --workspace --all-targets \
    --target x86_64-pc-windows-msvc --cross-compiler clang
```

passes for the **whole workspace**, the app included. The app's build (Tauri's, for its Windows icon and version)
runs `llvm-rc`, which neither Xcode nor Rust's tools have: it's taken from LLVM's own release (23.1.4,
`LLVM-23.1.4-macOS-ARM64.tar.xz`, its SHA-256 checked against the release's attestation), `bin/llvm-rc` alone
(self-contained, 0.8 MB), put beside `llvm-lib` in `~/.local/westplay-xwin-bin`. Clippy for Windows shows nothing in
the files the port changed (only the `as_chunks` suggestions in files it hasn't touched, below). **Not run from
here:** the tests, which are Windows programs; nor, so, the Windows-only tests (`#[cfg(windows)]`: the plain names,
UAC, a loaded DLL in use, TruckersMP's paths).

Other notes:
- This toolchain's clippy (1.99) suggests `as_chunks` in some files not touched here (mapdata, `tablet.rs`):
  left as they are.
