using System;
using System.IO;
using System.Drawing;
using System.Windows.Forms;
using System.Web.Script.Serialization;

// Use native callbacks so PowerShell's runspace cannot delay UIA provider calls.
public static class PabUiFixture {
 public static void Run(string directory, int nodeCount) {
  string ready=Path.Combine(directory,"ready.json");
  if(File.Exists(ready)) throw new InvalidOperationException("Fixture already used");
  using(var form=new Form()) {
   form.Text="PAB UI acceptance fixture";form.Size=new Size(640,580);form.StartPosition=FormStartPosition.CenterScreen;
   var edit=new TextBox {Name="fixture_input",AccessibleName="Fixture input"};edit.SetBounds(20,20,360,30);
   var check=new CheckBox {Text="Fixture option",AccessibleName="Fixture option"};check.SetBounds(20,60,200,30);
   var button=new Button {Text="Apply fixture",AccessibleName="Apply fixture"};button.SetBounds(20,100,200,35);
   var read=new TextBox {Text="unchanged",ReadOnly=true,AccessibleName="Fixture readonly"};read.SetBounds(20,150,200,30);
   var secret=new TextBox {Text="fixture-only-secret",UseSystemPasswordChar=true,AccessibleName="Fixture secure"};secret.SetBounds(20,190,200,30);
   var disabled=new Button {Text="Fixture disabled",AccessibleName="Fixture disabled",Enabled=false};disabled.SetBounds(20,230,200,30);
   var radio=new RadioButton {Text="Fixture radio",AccessibleName="Fixture radio"};radio.SetBounds(20,270,200,30);
   var list=new ListBox {AccessibleName="Fixture list"};list.Items.AddRange(new object[]{"Fixture first","Fixture second"});list.SetBounds(260,150,200,100);
   form.Controls.AddRange(new Control[]{edit,check,button,read,secret,disabled,radio,list});
   for(int i=0;i<2;i++) {var b=new Button {Text="Fixture duplicate",AccessibleName="Fixture duplicate"};b.SetBounds(20,320+40*i,200,30);form.Controls.Add(b);}
   for(int i=0;i<nodeCount;i++) {var label=new Label {Text="Budget node "+i,AccessibleName="Budget node "+i};label.SetBounds(260,280+i*22,180,20);form.Controls.Add(label);}
   var json=new JavaScriptSerializer();int clicks=0;
   button.Click+=(s,e)=> {clicks++;File.WriteAllText(Path.Combine(directory,"result.json"),json.Serialize(new {clicks,value=edit.Text,@checked=check.Checked,radio=radio.Checked,selected=list.SelectedIndex}));};
   DateTime started=DateTime.UtcNow;
   using(var timer=new Timer {Interval=100}) {
    timer.Tick+=(s,e)=> {
     if(File.Exists(Path.Combine(directory,"stop")) || (DateTime.UtcNow-started).TotalMinutes>=5) {form.Close();return;}
     string hang=Path.Combine(directory,"hang");
     if(File.Exists(hang)) {File.Delete(hang);System.Threading.Thread.Sleep(10000);}
    };
    form.Shown+=(s,e)=> {File.WriteAllText(ready,json.Serialize(new {pid=System.Diagnostics.Process.GetCurrentProcess().Id,hwnd=form.Handle.ToInt64()}));timer.Start();};
    Application.Run(form);
   }
  }
 }
}
