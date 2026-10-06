param([Parameter(Mandatory=$true)][string]$ResultDirectory, [ValidateRange(0,1000)][int]$NodeCount = 0)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
if (-not (Test-Path -LiteralPath $ResultDirectory -PathType Container)) { throw 'Create a fresh fixture directory first' }
$ready = Join-Path $ResultDirectory 'ready.json'
if (Test-Path -LiteralPath $ready) { throw 'Fixture directory already used' }
$form = New-Object System.Windows.Forms.Form
$form.Text = 'PAB UI acceptance fixture'
$form.Size = New-Object System.Drawing.Size(640,580)
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
    @{clicks=$script:clicks;value=$edit.Text;checked=$check.Checked;radio=$radio.Checked;selected=$list.SelectedIndex} |
        ConvertTo-Json | Set-Content -LiteralPath (Join-Path $ResultDirectory 'result.json') -Encoding UTF8
})
$form.Controls.AddRange(@($edit,$check,$button))
$readonly = New-Object System.Windows.Forms.TextBox
$readonly.AccessibleName = 'Fixture readonly'
$readonly.ReadOnly = $true
$readonly.Text = 'unchanged'
$readonly.SetBounds(20,150,200,30)
$secret = New-Object System.Windows.Forms.TextBox
$secret.AccessibleName = 'Fixture secure'
$secret.UseSystemPasswordChar = $true
$secret.Text = 'fixture-only-secret'
$secret.SetBounds(20,190,200,30)
$disabled = New-Object System.Windows.Forms.Button
$disabled.AccessibleName = 'Fixture disabled'
$disabled.Text = 'Fixture disabled'
$disabled.Enabled = $false
$disabled.SetBounds(20,230,200,30)
$radio = New-Object System.Windows.Forms.RadioButton
$radio.AccessibleName = 'Fixture radio'
$radio.Text = 'Fixture radio'
$radio.SetBounds(20,270,200,30)
$list = New-Object System.Windows.Forms.ListBox
$list.AccessibleName = 'Fixture list'
$list.Items.AddRange(@('Fixture first','Fixture second'))
$list.SetBounds(260,150,200,100)
$form.Controls.AddRange(@($readonly,$secret,$disabled,$radio,$list))
foreach ($index in 0..1) {
    $duplicate = New-Object System.Windows.Forms.Button
    $duplicate.AccessibleName = 'Fixture duplicate'
    $duplicate.Text = 'Fixture duplicate'
    $duplicate.SetBounds(20,(320+40*$index),200,30)
    $form.Controls.Add($duplicate)
}
for ($index=0; $index -lt $NodeCount; $index++) {
    $label=New-Object System.Windows.Forms.Label
    $label.Text="Budget node $index"
    $label.AccessibleName=$label.Text
    $label.SetBounds(260,(280+$index*22),180,20)
    $form.Controls.Add($label)
}
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
