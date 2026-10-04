param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][string]$Fixture
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$directory = Join-Path $env:TEMP ('dopaterm-background-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $directory | Out-Null
$config = Join-Path $directory 'config.toml'
[IO.File]::WriteAllText($config, "shell = '$Fixture'`nfont_family = 'Cascadia Code'`nfont_size = 14.0`nintensity = 'off'`nbackground = '#1d1f21'`n")
$oldScroll = $env:DOPA_BACKGROUND_SCROLL_DEMO
$oldSettings = $env:DOPA_SETTINGS_DEMO
$oldIme = $env:DOPA_IME_DEMO
$env:DOPA_SETTINGS_DEMO = $null
$env:DOPA_IME_DEMO = $null
try {
    foreach ($case in @('current', 'scrollback')) {
        $env:DOPA_BACKGROUND_SCROLL_DEMO = $null
        if ($case -eq 'scrollback') { $env:DOPA_BACKGROUND_SCROLL_DEMO = '1' }
        $path = Join-Path $directory "$case.png"
        & $Binary --config $config --no-fx --screenshot $path
        if ($LASTEXITCODE -ne 0 -or !(Test-Path $path)) { throw "$case screenshot failed" }
        $image = New-Object System.Drawing.Bitmap($path)
        try {
            for ($row = 0; $row -lt 20; $row++) {
                for ($column = 0; $column -lt 24; $column++) {
                    $pixel = $image.GetPixel($column * 8 + 1, $row * 18 + 1)
                    if ($pixel.R -lt 37 -or $pixel.G -lt 41 -or $pixel.B -lt 48) {
                        throw "$case dark background gap at row=$row column=$column rgb=$($pixel.R),$($pixel.G),$($pixel.B)"
                    }
                }
            }
        } finally { $image.Dispose() }
        Write-Output "$case : fullwidth and blank cell backgrounds OK"
    }
} finally {
    $env:DOPA_BACKGROUND_SCROLL_DEMO = $oldScroll
    $env:DOPA_SETTINGS_DEMO = $oldSettings
    $env:DOPA_IME_DEMO = $oldIme
}
Write-Output "screenshots: $directory"
