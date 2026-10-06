param([Parameter(Mandatory=$true)][string]$ResultDirectory)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
if (-not (Test-Path -LiteralPath $ResultDirectory -PathType Container)) { throw 'Create a fresh fixture directory first' }
$ready = Join-Path $ResultDirectory 'ready.json'
if (Test-Path -LiteralPath $ready) { throw 'Fixture directory already used' }
$form = New-Object System.Windows.Forms.Form
$form.Text = 'PAB UI acceptance fixture'
$form.Size = New-Object System.Drawing.Size(460,300)
$form.StartPosition = 'CenterScreen'
$edit = New-Object System.Windows.Forms.TextBox
$edit.AccessibleName = 'Fixture input'
$edit.Name = 'fixture_input'
$edit.SetBounds(20,20,360,30)
$check = New-Object System.Windows.Forms.CheckBox
$check.Text = 'Fixture option'
$check.AccessibleName = 'Fixture option'
$check.SetBounds(20,60,200,30)
$button = New-Object System.Windows.Forms.Button
$button.Text = 'Apply fixture'
$button.AccessibleName = 'Apply fixture'
$button.SetBounds(20,100,200,35)
$script:clicks = 0
$button.Add_Click({
    $script:clicks++
    @{clicks=$script:clicks;value=$edit.Text;checked=$check.Checked} |
        ConvertTo-Json | Set-Content -LiteralPath (Join-Path $ResultDirectory 'result.json') -Encoding UTF8
})
$form.Controls.AddRange(@($edit,$check,$button))
$script:started = [DateTime]::UtcNow
$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = 100
$timer.Add_Tick({
    if (Test-Path -LiteralPath (Join-Path $ResultDirectory 'stop')) { $form.Close(); return }
    if (Test-Path -LiteralPath (Join-Path $ResultDirectory 'hang')) {
        Remove-Item -LiteralPath (Join-Path $ResultDirectory 'hang')
        [Threading.Thread]::Sleep(10000)
    }
    if (([DateTime]::UtcNow-$script:started).TotalMinutes -ge 5) { $form.Close() }
})
$form.Add_Shown({
    @{pid=$PID;hwnd=$form.Handle.ToInt64()} | ConvertTo-Json | Set-Content -LiteralPath $ready
    $timer.Start()
})
try { [System.Windows.Forms.Application]::Run($form) }
finally { $timer.Dispose(); $form.Dispose() }
