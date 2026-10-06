param([Parameter(Mandatory=$true)][string]$ResultDirectory, [ValidateRange(0,1000)][int]$NodeCount = 0)
$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $ResultDirectory -PathType Container)) { throw 'Create a fresh fixture directory first' }
Add-Type -Path (Join-Path $PSScriptRoot 'ui_controls.cs') -ReferencedAssemblies System.Windows.Forms,System.Drawing,System.Web.Extensions
[PabUiFixture]::Run($ResultDirectory, $NodeCount)
