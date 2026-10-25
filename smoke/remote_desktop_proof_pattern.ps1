param([Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$root=[IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Path $root -Force|Out-Null
Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing @'
using System;using System.Drawing;using System.Windows.Forms;using System.Diagnostics;using System.IO;
public class ProofPattern:Form {
 readonly Stopwatch clock=Stopwatch.StartNew();readonly Timer timer=new Timer();readonly string root;long paints;
 public ProofPattern(string path){root=path;FormBorderStyle=FormBorderStyle.None;Bounds=Screen.PrimaryScreen.Bounds;TopMost=true;DoubleBuffered=true;ShowInTaskbar=true;Text="Pixels synthetic pixel proof";KeyPreview=true;KeyDown+=(s,e)=>{if(e.KeyCode==Keys.Escape)Close();};timer.Interval=100;timer.Tick+=(s,e)=>{if(clock.Elapsed.TotalMinutes>12||File.Exists(Path.Combine(root,"stop")))Close();else Invalidate();};timer.Start();}
 protected override void OnPaint(PaintEventArgs e){int frame=(int)(paints++%65536),w=ClientSize.Width,h=ClientSize.Height;
 for(int y=0;y<18;y++)for(int x=0;x<32;x++){
 int r,g,b;if(y==0||y==17){int bit=((frame>>(x%16))&1)^(x>=16?1:0);r=g=b=bit!=0?224:32;}
 else{r=32+(frame*3+x*29+y*17)%192;g=32+(frame*5+x*11+y*31)%192;b=32+(frame*7+x*37+y*13)%192;}
 using(var brush=new SolidBrush(Color.FromArgb(r,g,b)))e.Graphics.FillRectangle(brush,x*w/32,y*h/18,(x+1)*w/32-x*w/32,(y+1)*h/18-y*h/18);
 }
 if(paints%10==0)File.WriteAllText(Path.Combine(root,"heartbeat.json"),"{\"paints\":"+paints+",\"seconds\":"+clock.Elapsed.TotalSeconds.ToString("F3",System.Globalization.CultureInfo.InvariantCulture)+",\"width\":"+w+",\"height\":"+h+",\"synthetic_proof\":true}");
 }
}
'@
[Windows.Forms.Application]::EnableVisualStyles()
[Windows.Forms.Application]::Run([ProofPattern]::new($root))
