# Privacy policy

Persian Phonetic IME will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it.

- The input method makes no network connections and has no telemetry, analytics or crash reporting.
- Typed text is never logged. It is not written to any file or included in error messages.
- Files the program writes, all on your own machine:
  - `%APPDATA%\PersianPhoneticIme\config.toml`: your optional settings.
  - `%APPDATA%\PersianPhoneticIme\history.bin`: an optional word-history file used to rank word suggestions. You can clear it from the settings app, and uninstalling leaves it in place so you can delete it yourself.
  - `%TEMP%\PersianPhoneticIme\pime-setup.log`: written by the installer helper. It records installer steps and exit codes, never typed text.
- Uninstalling removes the program files. Your config and history stay until you delete them.

Release builds are not code signed. No code signing service is involved, and nothing is sent to one. Verify a download against the `.sha256` file attached to the release.
