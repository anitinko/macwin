MacWin for Windows
==================

MacWin shares one keyboard, mouse and clipboard between this PC and a Mac on
the same network.

Install
  1. Unpack the zip.
  2. Open PowerShell in the unpacked folder (in Explorer: File > Open Windows
     PowerShell) and run:

       powershell -ExecutionPolicy Bypass -File .\install.ps1

     MacWin is copied to %LOCALAPPDATA%\Programs\MacWin, starts at login and
     is allowed through Windows Firewall on private and domain networks
     (Windows asks for administrator rights once).
  3. Click the MacWin icon in the notification area (it may hide behind the
     ^ arrow) and choose "Pair with the other computer...". Do the same on
     the Mac: one computer shows a code, the other one asks for it. If they
     do not find each other, click "Pair by address..." in one code window
     and enter the address shown in the other one. Then say on which side
     the other computer's screens are.

Use
  Push the mouse over the shared screen edge to control the other computer;
  using this PC's own mouse or keyboard takes control back. Text, images and
  files copied on one computer can be pasted on the other.
  Emergency release: Ctrl+Alt+Shift+Esc.
  The icon menu pauses sharing, turns the clipboard off, changes the side
  and quits. In a new terminal, `macwin --help` lists the commands.

Requirements and limits
  - The network must be a private network (Settings > Network & Internet >
    the connection > Network profile type: Private).
  - The lock screen, UAC prompts and windows of programs that run as
    administrator do not accept input from MacWin.

Uninstall
  powershell -ExecutionPolicy Bypass -File "%LOCALAPPDATA%\Programs\MacWin\uninstall.ps1"
  Add -Purge to also delete the settings and the pairing.
