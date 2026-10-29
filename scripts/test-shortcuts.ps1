# Run only in a disposable Windows CI session without an existing app installation.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class ShortcutUi {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr window, int id);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr window);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, UIntPtr wparam, IntPtr lparam);
    [DllImport("user32.dll")] public static extern IntPtr SendMessageTimeout(IntPtr window, uint message, UIntPtr wparam, IntPtr lparam, uint flags, uint timeout, out UIntPtr result);
    [DllImport("user32.dll")] public static extern bool RegisterHotKey(IntPtr window, int id, uint modifiers, uint key);
    [DllImport("user32.dll")] public static extern bool UnregisterHotKey(IntPtr window, int id);
    private delegate bool EnumCallback(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumCallback callback, IntPtr data);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder text, int count);
    public static string DescribeWindows(uint processId) {
        var text = new StringBuilder();
        EnumWindows((window, data) => {
            uint pid; GetWindowThreadProcessId(window, out pid);
            if (pid == processId) {
                var title = new StringBuilder(512); GetWindowText(window, title, title.Capacity);
                text.AppendLine(window + " enabled=" + IsWindowEnabled(window) + " title=" + title);
            }
            return true;
        }, IntPtr.Zero);
        return text.ToString();
    }
}
'@

function Wait-For([scriptblock]$Condition, [string]$Description) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.Elapsed.TotalSeconds -lt 10) {
        $value = & $Condition
        if ($null -ne $value -and $value -ne [IntPtr]::Zero) { return $value }
        Start-Sleep -Milliseconds 100
    }
    throw "Timed out: $Description"
}
function Send-Ui([IntPtr]$Window, [uint32]$Message, [uint64]$Wparam = 0, [int64]$Lparam = 0) {
    $result = [UIntPtr]::Zero
    if ([ShortcutUi]::SendMessageTimeout($Window, $Message, [UIntPtr]$Wparam, [IntPtr]$Lparam, 2, 2000, [ref]$result) -eq [IntPtr]::Zero) {
        throw 'The app did not respond to a UI test message.'
    }
    return $result.ToUInt64()
}
function Open-Dialog {
    $main = [ShortcutUi]::FindWindow('SameAppSwitcherWnd', $null)
    [void][ShortcutUi]::PostMessage($main, 0x111, [UIntPtr]100, [IntPtr]::Zero)
    Wait-For { [ShortcutUi]::FindWindow('#32770', 'SameAppSwitcher - Shortcuts') } 'shortcut dialog'
}
function Read-Hotkey([IntPtr]$Dialog, [int]$Id) {
    Send-Ui ([ShortcutUi]::GetDlgItem($Dialog, $Id)) 0x402
}
function Set-Hotkey([IntPtr]$Dialog, [int]$Id, [uint64]$Value) {
    [void](Send-Ui ([ShortcutUi]::GetDlgItem($Dialog, $Id)) 0x401 $Value)
    [void](Send-Ui $Dialog 0x111 ($Id -bor (0x300 -shl 16)))
}
function Close-App {
    $main = [ShortcutUi]::FindWindow('SameAppSwitcherWnd', $null)
    if ($main -ne [IntPtr]::Zero) {
        [void][ShortcutUi]::PostMessage($main, 0x111, [UIntPtr]102, [IntPtr]::Zero)
    }
    if ($script:app) { $script:app.WaitForExit(5000) | Out-Null }
    if ($script:app -and !$script:app.HasExited) { throw 'App did not exit.' }
}

$settings = 'HKCU:\Software\SameAppSwitcher'
$app = $null
if ([ShortcutUi]::FindWindow('SameAppSwitcherWnd', $null) -ne [IntPtr]::Zero) { throw 'An app instance is already running.' }
$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\SameAppSwitcher')
if ($key) {
    try {
        if ($null -ne $key.GetValue('Shortcut') -or $null -ne $key.GetValue('Shortcuts')) { throw 'Use a clean session without saved shortcuts.' }
    } finally { $key.Dispose() }
}
try {
    if (!(Test-Path $settings)) { New-Item $settings -Force | Out-Null }
    New-ItemProperty $settings -Name Shortcut -Value ((3 -shl 16) -bor 0x51) -PropertyType DWord -Force | Out-Null
    $app = Start-Process (Resolve-Path dist/SameAppSwitcher.exe).Path -PassThru
    [void](Wait-For { [ShortcutUi]::FindWindow('SameAppSwitcherWnd', $null) } 'app startup')
    Write-Host 'App started; checking legacy migration.'
    $dialog = Open-Dialog
    if ((Read-Hotkey $dialog 1001) -ne 0x651 -or (Read-Hotkey $dialog 1005) -ne 0x751) { throw 'Legacy shortcut migration failed.' }
    Set-Hotkey $dialog 1005 0x651
    if ([ShortcutUi]::IsWindowEnabled([ShortcutUi]::GetDlgItem($dialog, 1))) { throw 'Duplicate shortcuts were accepted.' }
    Set-Hotkey $dialog 1005 0x657
    Write-Host 'Saving independent shortcuts.'
    [void](Send-Ui $dialog 0x111 1)
    if ([ShortcutUi]::IsWindow($dialog)) { throw 'Independent shortcut pair was not saved.' }
    $saved = [byte[]](Get-ItemPropertyValue $settings -Name Shortcuts)
    $expected = [BitConverter]::GetBytes([uint32]0x30051) + [BitConverter]::GetBytes([uint32]0x30057)
    if ([Convert]::ToBase64String($saved) -cne [Convert]::ToBase64String($expected)) { throw 'Saved shortcut pair was incorrect.' }

    Close-App
    Write-Host 'Restarting to check persistence.'
    $app = Start-Process (Resolve-Path dist/SameAppSwitcher.exe).Path -PassThru
    [void](Wait-For { [ShortcutUi]::FindWindow('SameAppSwitcherWnd', $null) } 'app restart')
    $dialog = Open-Dialog
    if ((Read-Hotkey $dialog 1001) -ne 0x651 -or (Read-Hotkey $dialog 1005) -ne 0x657) { throw 'Independent shortcuts did not survive restart.' }
    [void](Send-Ui ([ShortcutUi]::GetDlgItem($dialog, 1006)) 0xF1 1)
    [void](Send-Ui $dialog 0x111 1006)
    [void](Send-Ui $dialog 0x10)
    if ([ShortcutUi]::IsWindow($dialog)) { throw 'Closing the dialog did not destroy it.' }
    if ([ShortcutUi]::RegisterHotKey([IntPtr]::Zero, 999, 3, 0x57)) {
        [void][ShortcutUi]::UnregisterHotKey([IntPtr]::Zero, 999)
        throw 'Closing the dialog failed to restore the previous shortcut.'
    }
    if ([Convert]::ToBase64String([byte[]](Get-ItemPropertyValue $settings -Name Shortcuts)) -cne [Convert]::ToBase64String($saved)) {
        throw 'Cancel changed saved shortcuts.'
    }
    $dialog = Open-Dialog
    [void](Send-Ui $dialog 0x111 1004)
    if ((Read-Hotkey $dialog 1005) -ne ((Read-Hotkey $dialog 1001) -bor 0x100)) { throw 'Reset did not restore the default reverse shortcut.' }
    [void](Send-Ui $dialog 0x111 2)
} catch {
    Write-Host ($_ | Out-String)
    if ($app) { Write-Host ([ShortcutUi]::DescribeWindows([uint32]$app.Id)) }
    throw
} finally {
    try { Close-App } catch {
        Write-Warning $_
        if ($app -and !$app.HasExited) { Stop-Process -Id $app.Id -Force }
    }
    Remove-ItemProperty $settings -Name Shortcut -ErrorAction SilentlyContinue
    Remove-ItemProperty $settings -Name Shortcuts -ErrorAction SilentlyContinue
}
Write-Host 'Verified legacy migration, independent shortcuts, duplicate validation, persistence, reset, and Cancel/close recovery.'
