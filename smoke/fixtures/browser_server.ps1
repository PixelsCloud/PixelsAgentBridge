$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$body = [Convert]::FromBase64String('__HTML_BASE64__')
$listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
$listener.Start()
[pscustomobject]@{port=$listener.LocalEndpoint.Port;pid=$PID} | ConvertTo-Json -Compress
$clock = [Diagnostics.Stopwatch]::StartNew()
$incompleteRequests = 0
try {
    while ($clock.Elapsed.TotalSeconds -lt 600) {
        if (-not $listener.Pending()) { Start-Sleep -Milliseconds 50; continue }
        $client = $listener.AcceptTcpClient()
        try {
            $client.ReceiveTimeout = 2000
            $stream = $client.GetStream()
            $reader = [IO.StreamReader]::new($stream)
            $line = $reader.ReadLine()
            if (-not $line) { continue }
            $headers = 0
            while ($reader.ReadLine()) {
                $headers++
                if ($headers -gt 32) { throw [IO.InvalidDataException]::new('header budget') }
            }
            $ok = $line -match '^GET /browser.html(?:[? ]|$)'
            $bytes = if ($ok) { $body } else { [Text.Encoding]::UTF8.GetBytes('not found') }
            $status = if ($ok) { '200 OK' } else { '404 Not Found' }
            $header = [Text.Encoding]::ASCII.GetBytes("HTTP/1.1 $status`r`nContent-Type: text/html; charset=utf-8`r`nContent-Length: $($bytes.Length)`r`nConnection: close`r`n`r`n")
            $stream.Write($header, 0, $header.Length)
            $stream.Write($bytes, 0, $bytes.Length)
            $stream.Flush()
        } catch {
            # Chrome may preconnect without sending a request, or close a
            # connection during navigation. That client must not kill the server.
            $cause = $_.Exception.GetBaseException()
            if ($cause -isnot [Net.Sockets.SocketException] -and $cause -isnot [IO.IOException]) { throw }
            $incompleteRequests++
        } finally {
            $client.Close()
        }
    }
} finally {
    $listener.Stop()
    [pscustomobject]@{incomplete_requests=$incompleteRequests} | ConvertTo-Json -Compress
}
