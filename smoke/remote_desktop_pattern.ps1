param(
 [string]$OutputDirectory = (Join-Path $PSScriptRoot 'runs/rd-pattern'),
 [ValidateRange(30,3600)][int]$Seconds = 720
)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$root=[IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Path $root -Force | Out-Null
Start-Transcript -LiteralPath (Join-Path $root 'pattern-log.txt') -Force | Out-Null
Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing @'
using System;using System.Drawing;using System.Windows.Forms;using System.Diagnostics;using System.IO;
public class StabilityPattern:Form {
 readonly Stopwatch clock=Stopwatch.StartNew(); readonly Timer timer=new Timer(); long paints=0; string root; Bitmap background;
 public StabilityPattern(string path,int seconds){root=path;Text="PAB synthetic desktop stability test";FormBorderStyle=FormBorderStyle.None;Bounds=Screen.PrimaryScreen.Bounds;TopMost=true;DoubleBuffered=true;ShowInTaskbar=true;KeyPreview=true;KeyDown+=(s,e)=>{if(e.KeyCode==Keys.Escape)Close();};timer.Interval=10;timer.Tick+=(s,e)=>{if(clock.Elapsed.TotalSeconds>seconds||File.Exists(Path.Combine(root,"stop")))Close();else if(!File.Exists(Path.Combine(root,"freeze")))Invalidate();};timer.Start();}
 protected override void OnPaint(PaintEventArgs e){paints++;int w=ClientSize.Width,h=ClientSize.Height;if(background==null){background=new Bitmap(w,h);using(var g=Graphics.FromImage(background)){
 g.Clear(Color.FromArgb(245,247,250));g.TextRenderingHint=System.Drawing.Text.TextRenderingHint.ClearTypeGridFit;
 using(var title=new Font("Consolas",24,FontStyle.Bold))g.DrawString("PIXELS / REMOTE DESKTOP / 30 FPS TEST",title,Brushes.Black,24,22);
 int y=85;foreach(int size in new int[]{9,10,12,14,16,20,24}){using(var f=new Font("Consolas",size,FontStyle.Regular,GraphicsUnit.Pixel))g.DrawString("AaBb 0123456789 | Il1 O0 mwMW #@% []{}  size="+size+"px",f,Brushes.Black,30,y);y+=36;}
 using(var f=new Font("Microsoft YaHei UI",14,FontStyle.Regular,GraphicsUnit.Pixel))g.DrawString("\u8fdc\u7a0b\u684c\u9762\u6e05\u6670\u5ea6\u6d4b\u8bd5\uff1a\u8bbe\u5907\u8fde\u63a5\u3001\u7f16\u7801\u89e3\u7801\u3001\u7f51\u7edc\u901a\u4fe1\u3002",f,Brushes.Black,30,y);y+=40;
 Brush[] colors={Brushes.Red,Brushes.Green,Brushes.Blue,Brushes.Magenta};foreach(var b in colors){using(var f=new Font("Consolas",14,FontStyle.Regular,GraphicsUnit.Pixel))g.DrawString("Colored text / 0123456789 / RGB chroma",f,b,30,y);y+=30;}
 for(int n=1;n<=4;n++){int start=30+(n-1)*120;for(int x=0;x<80;x+=2*n)g.FillRectangle(Brushes.Black,start+x,y,n,75);using(var f=new Font("Consolas",13))g.DrawString(n+"px",f,Brushes.Black,start,y+80);}
 int right=Math.Max(570,w/2+25);for(int gy=80;gy<h-50;gy+=32)g.DrawLine(Pens.LightGray,right,gy,w-25,gy);for(int gx=right;gx<w-25;gx+=32)g.DrawLine(Pens.LightGray,gx,80,gx,h-50);
 using(var f=new Font("Consolas",15))g.DrawString("SYNTHETIC CONTENT ONLY | ESC closes | "+w+"x"+h,f,Brushes.Black,25,h-40);
 }}e.Graphics.DrawImageUnscaled(background,0,0);Motion(e.Graphics,w,h);
 if(paints==120){using(var image=(Bitmap)background.Clone()){using(var g=Graphics.FromImage(image))Motion(g,w,h);image.Save(Path.Combine(root,"source-reference.png"),System.Drawing.Imaging.ImageFormat.Png);}}
 if(paints%120==0)File.WriteAllText(Path.Combine(root,"heartbeat.json"),"{\"paints\":"+paints+",\"seconds\":"+clock.Elapsed.TotalSeconds.ToString("F3",System.Globalization.CultureInfo.InvariantCulture)+",\"width\":"+w+",\"height\":"+h+"}");
 }
 void Motion(Graphics g,int w,int h){int right=Math.Max(570,w/2+25);double seconds=clock.Elapsed.TotalSeconds;int span=Math.Max(50,w-right-85);int moving=right+(int)(seconds*220)%span;g.FillRectangle(Brushes.Teal,moving,120,55,Math.Max(80,h-300));g.FillEllipse(Brushes.DarkOrange,right+(int)(seconds*137)%span,h-160,65,65);using(var f=new Font("Consolas",22,FontStyle.Bold))g.DrawString("t="+seconds.ToString("F2")+"s paint="+paints,f,Brushes.Black,right,80);}
}
'@
[Windows.Forms.Application]::EnableVisualStyles()
[Windows.Forms.Application]::Run([StabilityPattern]::new($root,$Seconds))
