# Installer (WS8)

Per-machine WiX v5 MSI for Persian Phonetic IME, plus the `pime-setup` helper (`crates/pime-setup`).

**Never install the MSI or run `pime-setup /register`, `/unregister`, `/enable-user`, `/disable-user` or `/cleanup-old` on a development machine.** A broken text service breaks typing in every app. Test only in Windows Sandbox or a VM checkpoint. `pime-setup /status` is read-only and safe anywhere.

## Build

Prerequisites: Rust with `x86_64-pc-windows-msvc` and `i686-pc-windows-msvc`, the MSVC linker, and WiX v5 with the Util extension:

```
dotnet tool install --global wix --version 5.0.2
wix extension add -g WixToolset.Util.wixext/5.0.2
```

Then build:

```
powershell -ExecutionPolicy Bypass -File installer\build.ps1 -Version 0.1.0 -DllX64 <tip x64.dll> -DllX86 <tip x86.dll> [-Validate]
```

What `build.ps1` does (build only; it never runs msiexec or any registration):

1. Runs `cargo test -p pime-setup`, unless `-SkipTests`.
2. Builds `pime-setup` for x64 and x86, and `pime-settings` for x64, into `target-ws8`, with a static CRT and Control Flow Guard.
3. Checks that each payload DLL has the right bitness.
4. Stages the payload in `installer\out\stage\<ver>\` under versioned names.
5. Optionally signs the payload.
6. Runs `wix build`, then `wix msi validate` if `-Validate` is given, then optionally signs the MSI.

Output: `installer\out\PersianPhoneticIme-<ver>-x64.msi`.

- **Placeholder build:** with no `-DllX64`/`-DllX86`, the WS1 spike DLLs in `spike\dist` are used as stand-ins. The result is `...-x64-PLACEHOLDER.msi`, its product name says PLACEHOLDER, and it contains `PLACEHOLDER.txt`. It passes the spike's CLSID and profile to `pime-setup` (`--clsid/--profile`), so per-user enable can be exercised. It is for VM testing of the installer mechanics only.
- **GUIDs:** `build.ps1` reads the CLSID, profile, UpgradeCode and Active Setup id from `crates/pime-setup/src/guids.rs`, the single source of truth (frozen once released).
- **Version:** use `major.minor.build` (at most 255.255.65535). Every release needs a higher version, because each version installs side by side in its own folder.
- **ICE validation:** ICE43 and ICE57 are suppressed. They object to file-keyed shortcuts in `ProgramMenuFolder` in a package that *could* be per-user. This package is always per-machine, so those shortcuts land in the all-users Start menu.

### Signing (optional)

```
build.ps1 ... -CertThumbprint <SHA1 of a code-signing cert in your store> [-TimestampUrl <url>] [-SignTool <path to signtool.exe>]
```

This signs every staged DLL and EXE before packaging, then signs the MSI, using `signtool sign /sha1 <thumb> /fd SHA256 /tr <url> /td SHA256`. No keys or passwords live in the repo; the certificate comes from the Windows certificate store.

- Unsigned builds are for development only.
- Immersive hosts (Settings, Start search) are expected to ignore an unsigned TIP (docs/spike.md 4.9).
- Timestamping contacts the timestamp server, so it only happens when signing is requested.

## Layout

```
%ProgramFiles%\Persian Phonetic IME\<ver>\
  PersianPhoneticIme.<ver>.x64.dll   TIP 64-bit
  PersianPhoneticIme.<ver>.x86.dll   TIP 32-bit (same CLSID, WOW64 registry view)
  pime-setup.x64.exe                 registers the x64 DLL; per-user enable
  pime-setup.x86.exe                 registers the x86 DLL
  pime-settings.exe                  settings app (WS7)
Start menu: "Persian Phonetic IME Settings", "Add Persian Phonetic keyboard for me"
```

Presets are compiled into the DLL, so the package installs no data files. The MSI needs 64-bit Windows 10 or later; 32-bit Windows is not supported in v1.

## What the MSI does

| When | Action | Context |
|---|---|---|
| install, repair, upgrade (after InstallFiles) | `pime-setup.x86 /register <x86 dll>`, then `pime-setup.x64 /register <x64 dll>`. Each calls the DLL's `DllRegisterServer` (InprocServer32 in its own registry view, RegisterProfile, categories). x64 runs last so the profile's icon file is the x64 DLL. | deferred, SYSTEM, fails the install on error |
| rollback of the above | Re-registers the previous version's DLL (`PREVDLLX64`/`PREVDLLX86`, read from InprocServer32 before install), or unregisters if there was none | rollback, SYSTEM |
| install (before InstallFinalize) | `/cleanup-old "<root>." <ver>`: deletes older version folders, or schedules in-use files for deletion at reboot (MoveFileEx) | deferred, SYSTEM, errors ignored |
| fresh install (after InstallFinalize) | `/enable-user`: InstallLayoutOrTip for the installing user | immediate, impersonated |
| uninstall (before InstallInitialize) | `/disable-user` (ILOT_UNINSTALL) for the uninstalling user | immediate, impersonated |
| uninstall (before RemoveFiles) | `/unregister` x86, then x64 (`DllUnregisterServer`); rollback re-registers | deferred, SYSTEM |

Every action runs through WixQuietExec64 (Util extension). There is no console window, and the helper's output goes into the MSI log (`msiexec /i x.msi /l*v install.log`). The helper also appends to `%TEMP%\PersianPhoneticIme\pime-setup.log`. For SYSTEM actions that is the system temp folder (usually `C:\Windows\Temp`). The log never contains typed text.

### Upgrade

`MajorUpgrade Schedule="afterInstallExecute"`:

1. The new version's files are installed into a new versioned folder.
2. The new version is registered, which repoints `InprocServer32` at the new versioned DLL.
3. Only then is the old product removed. Its unregister and disable steps are skipped (`UPGRADINGPRODUCTCODE`), so the CLSID, the profile and every user's list entry survive.

Old DLLs that running apps still have loaded are deleted at reboot. Windows Installer handles in-use files itself, and `/cleanup-old` is a safety net. Restart Manager is disabled (`MSIRESTARTMANAGERCONTROL=Disable`): the DLL is loaded in nearly every process, so a "close these apps" prompt would be useless, and versioned names make closing apps unnecessary. Downgrades are blocked.

### Uninstall

Uninstall disables the IME for the uninstalling user, unregisters both DLLs, and removes the files and the Start menu shortcuts. The empty product folder is removed. In other users' accounts the language-list entry stays, but it is inert once the profile is unregistered.

User data is left alone: `%APPDATA%\PersianPhoneticIme` and `HKCU\Software\PersianPhoneticIme`.

## Per-user enable (decision)

`InstallLayoutOrTip` changes the **current user's** input-method list. From a SYSTEM custom action it would change SYSTEM's profile and do nothing for the user. The installer therefore does three things:

1. **Installing user:** an immediate, impersonated action after InstallFinalize, once the machine-wide registration is committed.
   - Runs only on a fresh install.
   - Controlled by `ENABLE_FOR_INSTALLING_USER` (default `1`).
   - Skipped when the installer runs as SYSTEM.
   - Upgrades do not re-add the keyboard.
2. **Other users (opt-in):** `msiexec /i ... ENABLE_ALL_USERS=1` registers an Active Setup entry.
   - Windows then runs `pime-setup.x64.exe /enable-user /quiet` once per user at their next logon, in that user's context.
   - The Active Setup version stays at `1,0,0,0`, so upgrades do not re-add the keyboard for users who removed it.
   - It is off by default so a shared machine does not get Persian added to every account.
   - Use it for managed (SYSTEM) deployments and for "over-the-shoulder" UAC installs, where the installing account is an admin rather than the person who will type.
3. **Manual:** the Start menu shortcut "Add Persian Phonetic keyboard for me", `pime-setup.x64.exe /enable-user`, or Windows Settings > Time & language > Language > Persian > Options > Add a keyboard > Persian Phonetic.

**Descriptor spelling:** `/enable-user` tries the descriptor `0429:{CLSID}{Profile}` first, then `0x0429:...`, and logs which one was accepted. Evidence so far: the per-user list on a Windows 10 machine stores entries as `0411:{CLSID}{Profile}` (no `0x`), so the first spelling is expected to work.

## pime-setup reference

```
pime-setup /register <tip.dll>      DllRegisterServer (DLL must match the helper's bitness)
pime-setup /unregister <tip.dll>    DllUnregisterServer
pime-setup /enable-user             InstallLayoutOrTip(<descriptor>, 0) for the current user
pime-setup /disable-user            InstallLayoutOrTip(<descriptor>, ILOT_UNINSTALL)
pime-setup /status                  read-only report
pime-setup /cleanup-old <root> <keep-version>
options: /quiet, --clsid {GUID} --profile {GUID}
```

| Exit code | Meaning |
|---|---|
| 0 | ok (`/status`: registered) |
| 1 | other error |
| 2 | usage |
| 3 | bad DLL path, not a DLL, or wrong bitness |
| 4 | LoadLibrary failed |
| 5 | export missing |
| 6 | DllRegisterServer or DllUnregisterServer returned a failure HRESULT |
| 7 | InstallLayoutOrTip failed (input.dll missing, or FALSE for both spellings) |
| 8 | cleanup incomplete |
| 10 | `/status`: not (fully) registered |

`/register` and `/unregister` check the DLL's PE header first, so the x86 DLL handed to the x64 helper fails with exit 3 before anything is loaded. Both helpers embed an `asInvoker` manifest: without it, Windows' installer-detection heuristic auto-elevates 32-bit EXEs whose name contains "setup".

## VM test checklist

Run this in Windows Sandbox (fast, Windows 10) and in a Hyper-V VM with a checkpoint (Windows 10 and 11, a 32-bit host, Word). Install with `msiexec /i <msi> /l*v C:\install.log`. Keep a second keyboard ready (Win+Space). The kill switch is `HKCU\Software\PersianPhoneticIme\Disabled=1`.

| # | Check | Result |
|---|---|---|
| 1 | Fresh install succeeds. `install.log` shows each pime-setup step with exit 0. `pime-setup.x64.exe /status` exits 0 and shows both COM views pointing at the versioned DLLs | |
| 2 | Installing user: the IME appears under Win+Space without a logoff. The log records which ILOT spelling was accepted (copy it into docs/spike.md) | |
| 3 | Typing works in Notepad (x64) and in `C:\Windows\SysWOW64\notepad.exe` (x86 DLL) | |
| 4 | `ENABLE_ALL_USERS=1`: a second user gets the IME at first logon, with at most a brief console flash. `ENABLE_FOR_INSTALLING_USER=0` skips step 2 | |
| 5 | Upgrade 0.1.0 -> 0.1.1 while Notepad has the IME loaded: no close-apps prompt. InprocServer32 (both views) points at the 0.1.1 DLLs. The IME keeps working and stays in the user's list. A reboot is requested if the old DLL was in use. After the reboot the `0.1.0` folder is gone (check `PendingFileRenameOperations` before rebooting) | |
| 6 | Rollback: force a failure after registration (for example, a test build whose `/register` exit is nonzero for the x64 DLL). The previous version must still be registered and working afterwards | |
| 7 | Repair (`msiexec /f`) re-registers without errors | |
| 8 | Uninstall: the IME disappears from the installing user's list. `/status` (run from a copy of the helper) reports everything absent, and the CLSID keys in both views, `HKLM\...\CTF\TIP\{CLSID}` and the Active Setup key are gone. The Program Files folder is gone, or pending reboot. Record whether a per-user `HKCU\...\CTF\TIP\{CLSID}` key survives | |
| 9 | Downgrade attempt is blocked with the message | |
| 10 | Silent install as SYSTEM (`psexec -s msiexec /i ... /qn ENABLE_ALL_USERS=1`): no ILOT runs as SYSTEM, and Active Setup enables users at logon | |
| 11 | Signed build (`-CertThumbprint`): signatures valid on the DLLs, EXEs and MSI; behaviour in Settings search | |
| 12 | No Application Error events naming PersianPhoneticIme or pime-setup | |
