param(
    [Parameter(Mandatory = $true)]
    [string]$ShortcutPath
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $ShortcutPath -PathType Leaf)) {
    throw "Application shortcut not found: $ShortcutPath"
}

# Ask the interactive Explorer shell to launch the shortcut. The installer is
# elevated, while the desktop app must run as the signed-in user.
$shell = New-Object -ComObject Shell.Application
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($ShortcutPath)
$explorerWindow = @($shell.Windows()) |
    Where-Object { $_.FullName -like '*\explorer.exe' } |
    Select-Object -First 1

if ($null -ne $explorerWindow) {
    $explorerWindow.Document.Application.ShellExecute(
        $shortcut.TargetPath,
        $shortcut.Arguments,
        '',
        'open',
        1
    )
} else {
    # Explorer is single-instance and routes the request to the interactive shell.
    Start-Process -FilePath (Join-Path $env:WINDIR 'explorer.exe') -ArgumentList "`"$ShortcutPath`""
}
