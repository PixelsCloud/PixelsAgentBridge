param([Parameter(Mandatory=$true)][string]$Action,[int]$Port=39235,[string]$Target='main',[string]$Expression='',[string]$Code='',[int]$Width=640,[int]$Height=360,[int]$Variant=1)
$ErrorActionPreference='Stop'
if($env:COMPUTERNAME -notin @('DESKTOP-N3GIEVQ','WIN-RACIRV0PP7C')){throw 'Designated remote test devices only'}
$root='C:\ProgramData\PixelsAgentBridgeAcceptance\image-clipboard-20261029'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -TypeDefinition @'
using System;using System.Drawing;using System.Drawing.Imaging;using System.Runtime.InteropServices;using System.Collections.Generic;using System.Text;
public static class ClipLab {
 public delegate bool EnumProc(IntPtr h,IntPtr p);
 [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f,IntPtr p);
 [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h,EnumProc f,IntPtr p);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h,StringBuilder b,int n);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h,StringBuilder b,int n);
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int n);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h,out Rect r);
 [StructLayout(LayoutKind.Sequential)]public struct Rect{public int L,T,R,B;}
 public static IntPtr Find(string title,string cls){IntPtr found=IntPtr.Zero;EnumWindows((h,p)=>{var b=new StringBuilder(512);GetWindowText(h,b,512);if(b.ToString().Replace(" ","").Contains(title)){if(cls==""){found=h;}else{EnumChildWindows(h,(c,q)=>{var n=new StringBuilder(128);GetClassName(c,n,128);if(n.ToString()==cls)found=c;return true;},IntPtr.Zero);}}return true;},IntPtr.Zero);return found;}
 public static void Key(IntPtr h,int k,bool down){if(!PostMessage(h,down?0x100u:0x101u,(IntPtr)k,(IntPtr)(down?0:unchecked((int)0xc0000001))))throw new Exception("key post failed");}
 public static Bitmap Pattern(int w,int h,int variant){var b=new Bitmap(w,h,PixelFormat.Format32bppArgb);var d=b.LockBits(new Rectangle(0,0,w,h),ImageLockMode.WriteOnly,PixelFormat.Format32bppArgb);byte[] data=new byte[Math.Abs(d.Stride)*h];for(int y=0;y<h;y++)for(int x=0;x<w;x++){int i=y*d.Stride+x*4;data[i]=(byte)((x+y+variant)%256);data[i+1]=(byte)((y+variant)%256);data[i+2]=(byte)((x+variant)%256);data[i+3]=(byte)(x<w/2?255:128);}Marshal.Copy(data,0,d.Scan0,data.Length);b.UnlockBits(d);return b;}
 public static string Pixels(Bitmap b){var d=b.LockBits(new Rectangle(0,0,b.Width,b.Height),ImageLockMode.ReadOnly,PixelFormat.Format32bppArgb);byte[] bytes=new byte[Math.Abs(d.Stride)*b.Height];Marshal.Copy(d.Scan0,bytes,0,bytes.Length);b.UnlockBits(d);using(var sha=System.Security.Cryptography.SHA256.Create()){return BitConverter.ToString(sha.ComputeHash(bytes)).Replace("-","").ToLowerInvariant();}}
}
'@ -ReferencedAssemblies System.Drawing,System.Windows.Forms
function Board($Request,[byte[]]$Payload=@()){
 $si=New-Object Diagnostics.ProcessStartInfo; $si.FileName='C:\Program Files\PixelsAgentBridge\pab-desktop.exe';$si.Arguments='--media-worker clipboard';$si.UseShellExecute=$false;$si.CreateNoWindow=$true;$si.RedirectStandardInput=$true;$si.RedirectStandardOutput=$true
 $p=New-Object Diagnostics.Process;$p.StartInfo=$si;[void]$p.Start()
 try{
  $w=New-Object IO.BinaryWriter($p.StandardInput.BaseStream);$r=New-Object IO.BinaryReader($p.StandardOutput.BaseStream)
  $bytes=[Text.Encoding]::UTF8.GetBytes(($Request|ConvertTo-Json -Compress -Depth 8));$w.Write([uint32]$bytes.Length);$w.Write($bytes);if($Payload.Length){$w.Write($Payload)};$w.Flush()
  $size=$r.ReadUInt32();if($size -gt 6292480){throw 'Oversized response'};$v=[Text.Encoding]::UTF8.GetString($r.ReadBytes($size))|ConvertFrom-Json
  if($null -ne $v.Err){throw [string]$v.Err};$s=$v.Ok
  if($s.image){$s.image|Add-Member -NotePropertyName data -NotePropertyValue ($r.ReadBytes([int]$s.image.bytes))}
  return $s
 }finally{if(!$p.HasExited){$p.Kill()};$p.Dispose()}
}
switch($Action){
 'cdp' {
  $pages=Invoke-RestMethod "http://127.0.0.1:$Port/json/list"
  $page=@($pages|Where-Object{ $_.type -eq 'page' -and $(if($Target -eq 'viewer'){$_.url -like '*desktopViewer=*'}elseif($Target -eq 'browser'){$_.url -like '*image-clipboard-lab.html*'}else{$_.url -like '*tauri*' -and $_.url -notlike '*desktopViewer=*'})})
  if($page.Count -ne 1){throw "Expected one owned $Target page, got $($page.Count)"}
  $ws=New-Object Net.WebSockets.ClientWebSocket;$cancel=New-Object Threading.CancellationTokenSource;$cancel.CancelAfter(40000)
  try{
   $ws.ConnectAsync([Uri]$page[0].webSocketDebuggerUrl,$cancel.Token).GetAwaiter().GetResult()
   $data=[Text.Encoding]::UTF8.GetBytes((@{id=1;method='Runtime.evaluate';params=@{expression=$Expression;userGesture=$true;awaitPromise=$true;returnByValue=$true}}|ConvertTo-Json -Compress -Depth 10))
   $ws.SendAsync([ArraySegment[byte]]::new($data),[Net.WebSockets.WebSocketMessageType]::Text,$true,$cancel.Token).GetAwaiter().GetResult()
   $buffer=New-Object byte[] 65536;$ms=New-Object IO.MemoryStream
   do{$msg=$ws.ReceiveAsync([ArraySegment[byte]]::new($buffer),$cancel.Token).GetAwaiter().GetResult();$ms.Write($buffer,0,$msg.Count)}while(!$msg.EndOfMessage)
   $result=[Text.Encoding]::UTF8.GetString($ms.ToArray())|ConvertFrom-Json
   if($result.error -or $result.result.exceptionDetails){throw ($result|ConvertTo-Json -Depth 8 -Compress)}
   $result.result.result.value|ConvertTo-Json -Depth 16 -Compress
  }finally{$ws.Dispose();$cancel.Dispose()}
 }
 'set' {
  $b=[ClipLab]::Pattern($Width,$Height,$Variant);$ms=New-Object IO.MemoryStream;$b.Save($ms,[Drawing.Imaging.ImageFormat]::Png);$png=$ms.ToArray()
  $s=Board @{operation='write_image';image=@{width=$Width;height=$Height;bytes=$png.Length};expected_revision=$null} $png
  @{width=$Width;height=$Height;variant=$Variant;bytes=$png.Length;pixels=[ClipLab]::Pixels($b);revision=$s.revision}|ConvertTo-Json -Compress
  $b.Dispose();$ms.Dispose()
 }
 'read' {
  $s=Board @{operation='read';after=$null};$result=@{revision=$s.revision;kind='other'}
  if($s.image){$ms=[IO.MemoryStream]::new([byte[]]$s.image.data);$b=[Drawing.Bitmap]::new($ms);$result.kind='image';$result.width=$b.Width;$result.height=$b.Height;$result.pixels=[ClipLab]::Pixels($b);$b.Dispose();$ms.Dispose()}elseif($null -ne $s.text){$result.kind='text';$result.length=$s.text.Length}
  $result|ConvertTo-Json -Compress
 }
 'keys' {
  $h=[ClipLab]::Find($Code,'PixelsRemoteDesktopSurface');$top=[ClipLab]::Find($Code,'');if($h -eq [IntPtr]::Zero){throw 'Viewer surface missing'}
  [void][ClipLab]::SetForegroundWindow($top)
  foreach($k in @(@(0x11,$true),@(0x56,$true),@(0x56,$false),@(0x11,$false))){[ClipLab]::Key($h,$k[0],$k[1])}
  @{posted=$true}|ConvertTo-Json -Compress
 }
 'click' {
  $h=[ClipLab]::Find($Code,'PixelsRemoteDesktopSurface');$top=[ClipLab]::Find($Code,'');if($h -eq [IntPtr]::Zero){throw 'Viewer surface missing'}
  [void][ClipLab]::ShowWindow($top,9);[void][ClipLab]::SetForegroundWindow($top);$r=New-Object ClipLab+Rect;[void][ClipLab]::GetClientRect($h,[ref]$r)
  $xy=([int]($r.R/2) -band 65535) -bor ([int]($r.B/2) -shl 16)
  [void][ClipLab]::PostMessage($h,0x200,[IntPtr]::Zero,[IntPtr]$xy);[void][ClipLab]::PostMessage($h,0x201,[IntPtr]1,[IntPtr]$xy);[void][ClipLab]::PostMessage($h,0x202,[IntPtr]::Zero,[IntPtr]$xy)
  @{clicked=$true}|ConvertTo-Json -Compress
 }
 default {throw 'Unknown test action'}
}
