param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][string]$NativeTest,
    [Parameter(Mandatory = $true)][string]$LinuxTest,
    [string]$Distro = 'Ubuntu'
)
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class MouseMessages {
    private delegate bool EnumWindow(IntPtr window, IntPtr param);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindow callback, IntPtr param);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetClassNameW(IntPtr window, System.Text.StringBuilder name, int size);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll", SetLastError = true)] private static extern IntPtr SendMessageTimeoutW(IntPtr window, uint message, IntPtr wParam, IntPtr lParam, uint flags, uint timeout, out UIntPtr result);
    public static void Send(IntPtr window, uint message, IntPtr wParam, IntPtr lParam) {
        UIntPtr result;
        if (SendMessageTimeoutW(window, message, wParam, lParam, 2, 2000, out result) == IntPtr.Zero)
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
    }
    public static IntPtr FindWindow(uint pid) {
        IntPtr result = IntPtr.Zero;
        EnumWindows((window, param) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            var name = new System.Text.StringBuilder(256);
            GetClassNameW(window, name, name.Capacity);
            if (owner == pid && IsWindowVisible(window) && name.ToString() == "Window Class") { result = window; return false; }
            return true;
        }, IntPtr.Zero);
        return result;
    }
}
'@
$directory = Join-Path $env:TEMP ('dopaterm-mouse-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $directory | Out-Null
function Read-Events([string]$path) {
    if (!(Test-Path $path)) { return '' }
    $stream = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
    $reader = New-Object IO.StreamReader($stream)
    try { return $reader.ReadToEnd() } finally { $reader.Dispose() }
}
$oldLog = $env:MOUSE_TEST_LOG
$oldWslEnv = $env:WSLENV
$oldDemo = $env:DOPA_MOUSE_DEMO
$env:DOPA_MOUSE_DEMO = $null
$env:WSLENV = (@($oldWslEnv, 'MOUSE_TEST_LOG/p') | Where-Object { $_ }) -join ':'
try {
    foreach ($case in @('native', 'wsl', 'powershell-wsl')) {
        $log = Join-Path $directory "$case.log"
        $env:MOUSE_TEST_LOG = $log
        $config = Join-Path $directory "$case.toml"
        $arguments = "shell_args = ['-d', '$Distro', '--', '$LinuxTest']"
        $shell = 'wsl.exe'
        if ($case -eq 'native') {
            $shell = $NativeTest
            $arguments = 'shell_args = []'
        } elseif ($case -eq 'powershell-wsl') {
            $shell = 'powershell.exe'
            $arguments = "shell_args = ['-NoProfile', '-Command', 'wsl.exe -d $Distro -- $LinuxTest']"
        }
        [IO.File]::WriteAllText($config, "shell = '$shell'`n$arguments`nintensity = 'off'`n")
        $process = Start-Process -FilePath $Binary -ArgumentList @('--config', ('"' + $config + '"')) -NoNewWindow -PassThru
        try {
            $deadline = [DateTime]::UtcNow.AddSeconds(20)
            do {
                Start-Sleep -Milliseconds 100
                $process.Refresh()
                $window = [MouseMessages]::FindWindow($process.Id)
                if ($process.HasExited) { throw "$case exited before mouse input" }
            } while (($window -eq 0 -or (Read-Events $log) -notmatch 'READY') -and [DateTime]::UtcNow -lt $deadline)
            if ($window -eq 0 -or (Read-Events $log) -notmatch 'READY') { throw "$case did not become ready" }
            Start-Sleep -Milliseconds 300
            $position = [IntPtr]((45 -shl 16) -bor 36)
            [MouseMessages]::Send($window, 0x0200, [IntPtr]::Zero, $position) | Out-Null
            [MouseMessages]::Send($window, 0x0201, [IntPtr]1, $position) | Out-Null
            [MouseMessages]::Send($window, 0x0200, [IntPtr]1, [IntPtr]((63 -shl 16) -bor 52)) | Out-Null
            [MouseMessages]::Send($window, 0x0202, [IntPtr]::Zero, $position) | Out-Null
            [MouseMessages]::Send($window, 0x020A, [IntPtr](120 -shl 16), $position) | Out-Null
            $deadline = [DateTime]::UtcNow.AddSeconds(5)
            do {
                Start-Sleep -Milliseconds 100
                $events = Read-Events $log
            } while ($events -notmatch 'ScrollUp' -and [DateTime]::UtcNow -lt $deadline)
            foreach ($expected in @('Moved', 'Down\(Left\)', 'Drag\(Left\)', 'Up\(Left\)', 'ScrollUp')) {
                if ($events -notmatch $expected) { throw "$case missing $expected : $events" }
            }
            Write-Output "$case : move, press, drag, release, wheel OK"
        } finally {
            if (!$process.HasExited) {
                [MouseMessages]::PostMessageW($window, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
                if (!$process.WaitForExit(5000)) { $process.Kill() }
            }
        }
    }
} finally {
    $env:MOUSE_TEST_LOG = $oldLog
    $env:WSLENV = $oldWslEnv
    $env:DOPA_MOUSE_DEMO = $oldDemo
}
Write-Output "logs: $directory"
