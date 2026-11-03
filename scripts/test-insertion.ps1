param([string]$Targets = 'winforms,edge')
$ErrorActionPreference = 'Stop'
# Interactive: opens disposable windows and types into them. Run after a debug test build.
$root = Split-Path -Parent $PSScriptRoot
$binary = Get-ChildItem "$root\target\debug\deps\articulate-*.exe" | Sort-Object LastWriteTime | Select-Object -Last 1
$env:PATH = "$root\.local\native\install\bin;$root\target\debug;" + $env:PATH
$env:TRANSCRIBE_UI_TEST = '1'
"binary: $($binary.Name) $($binary.LastWriteTime)"

function Wait-Window($process, $title) {
    for ($i = 0; $i -lt 60; $i++) {
        Start-Sleep -Milliseconds 500
        $found = Get-Process | Where-Object { $_.MainWindowTitle -like "$title*" } | Select-Object -First 1
        if ($found -and $found.MainWindowHandle -ne 0) { return $found }
    }
    throw "Window '$title' did not appear"
}

foreach ($target in $Targets.Split(',')) {
    "===== $target"
    $profile = $null
    if ($target -eq 'winforms') {
        $code = "Add-Type -AssemblyName System.Windows.Forms; `$f = New-Object Windows.Forms.Form; `$f.Text = 'ArticulateInsertTest'; `$f.Width = 600; `$f.Height = 300; `$t = New-Object Windows.Forms.TextBox; `$t.Multiline = `$true; `$t.Dock = 'Fill'; `$f.Controls.Add(`$t); `$f.Add_Shown({ `$t.Focus() }); [Windows.Forms.Application]::Run(`$f)"
        $process = Start-Process powershell -ArgumentList '-NoProfile', '-Command', $code -PassThru
    } else {
        $profile = Join-Path $env:TEMP "articulate-e2e-edge-$PID"
        New-Item -ItemType Directory -Force $profile | Out-Null
        $page = Join-Path $profile 'page.html'
        # edge: a plain text area. edge-rich: an empty rich-text editor laid out
        # like ProseMirror or Lexical (a contenteditable with one empty paragraph).
        $body = if ($target -eq 'edge-rich') {
            '<div id="e" contenteditable="true" role="textbox" aria-multiline="true" style="width:90%;height:200px;border:1px solid"><p><br></p></div><script>document.getElementById("e").focus()</script>'
        } else {
            '<textarea autofocus style="width:90%;height:200px"></textarea>'
        }
        Set-Content -LiteralPath $page -Value "<!doctype html><title>ArticulateInsertTest</title>$body"
        $page = 'file:///' + $page.Replace('\','/')
        $process = Start-Process 'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe' -ArgumentList "--user-data-dir=$profile\data", '--no-first-run', '--no-default-browser-check', '--new-window', $page -PassThru
    }
    try {
        $window = Wait-Window $process 'ArticulateInsertTest'
        Start-Sleep -Seconds 2
        $env:ARTICULATE_E2E_HWND = [string]$window.MainWindowHandle.ToInt64()
        & $binary.FullName 'integration::tests::shortcut_then_insertion_into_a_disposable_window' --ignored --exact --nocapture --test-threads=1 2>&1
        "exit: $LASTEXITCODE"
    } finally {
        Get-Process | Where-Object { $_.MainWindowTitle -like 'ArticulateInsertTest*' } | Stop-Process -ErrorAction SilentlyContinue
        if ($profile) {
            Start-Sleep -Seconds 2
            Get-CimInstance Win32_Process -Filter "Name='msedge.exe'" | Where-Object { $_.CommandLine -like "*$profile*" } | ForEach-Object { Stop-Process -Id $_.ProcessId -ErrorAction SilentlyContinue }
            Start-Sleep -Seconds 1
            Remove-Item $profile -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
