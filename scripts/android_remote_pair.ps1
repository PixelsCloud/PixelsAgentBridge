param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][Guid]$RunId,
    [Parameter(Mandatory = $true)][string]$ApkSha256,
    [ValidateSet('pair', 'check-old')][string]$Action = 'pair'
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$resolvedRoot = (Resolve-Path -LiteralPath $Root).Path
if ((Split-Path -Leaf $resolvedRoot) -ne $RunId.ToString()) { throw 'Wrong pairing session directory' }
if ($ApkSha256 -notmatch '^[0-9a-f]{64}$') { throw 'Invalid APK digest' }
$privateKey = Join-Path $resolvedRoot 'pair-private.xml'
$envelope = Join-Path $resolvedRoot 'pair-sealed.bin'
$rsa = [Security.Cryptography.RSACng]::new()
try {
    $rsa.FromXmlString([IO.File]::ReadAllText($privateKey))
    $plainBytes = $rsa.Decrypt([IO.File]::ReadAllBytes($envelope),
        [Security.Cryptography.RSAEncryptionPadding]::OaepSHA256)
    $plain = [Text.Encoding]::UTF8.GetString($plainBytes)
    $credential = $plain | ConvertFrom-Json
    if ($credential.run_id -ne $RunId.ToString()) { throw 'Pairing envelope session mismatch' }
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = Join-Path $resolvedRoot 'python\python.exe'
    $script = Join-Path $resolvedRoot 'scripts\android_remote_operator.py'
    $startInfo.Arguments = '"' + $script + '" ' + $Action + ' --root "' + $resolvedRoot +
        '" --run-id ' + $RunId.ToString() + ' --apk-sha256 ' + $ApkSha256
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
    $startInfo.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) { throw 'Could not start isolated pairing CLI' }
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $process.StandardInput.Write($plain)
    $process.StandardInput.Close()
    $process.WaitForExit()
    $safeOutput = ($stdout.Result + $stderr.Result).Replace([string]$credential.password, '[redacted]')
    Write-Output $safeOutput
    if ($process.ExitCode -ne 0) { throw 'Remote normal pairing failed; no plaintext credential retained' }
} finally {
    $rsa.Dispose()
    if ($null -ne $plainBytes) { [Array]::Clear($plainBytes, 0, $plainBytes.Length) }
    $plain = $null
    $credential = $null
    Remove-Item -LiteralPath $envelope -ErrorAction SilentlyContinue
    if ($Action -eq 'pair') {
        Remove-Item -LiteralPath $privateKey -ErrorAction SilentlyContinue
    }
}
