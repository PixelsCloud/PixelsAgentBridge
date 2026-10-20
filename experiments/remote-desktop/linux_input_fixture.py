"""Bounded generated fullscreen fixture; aggregate only clicks and Ctrl events."""
import json,os,pathlib,sys,time
os.environ.update(DBUS_SESSION_BUS_ADDRESS='unix:path=/run/user/1000/bus',XDG_RUNTIME_DIR='/run/user/1000',GDK_BACKEND='wayland')
import gi
gi.require_version('Gtk','3.0');from gi.repository import Gtk,Gdk,GLib
root=pathlib.Path(sys.argv[1]).resolve(strict=True)
assert root.parent==pathlib.Path('/tmp') and root.name.startswith('pab-linux-viewer-') and root.stat().st_uid==os.getuid()
os.umask(0o077)
counts={'clicks':0,'releases':0,'ctrl_down':0,'ctrl_up':0,'ctrl_held':False,'left_held':False,'last_x':None,'last_y':None}
window=Gtk.Window(title='Pixels isolated remote desktop acceptance');area=Gtk.DrawingArea();area.set_can_focus(True)
area.add_events(Gdk.EventMask.BUTTON_PRESS_MASK|Gdk.EventMask.BUTTON_RELEASE_MASK|Gdk.EventMask.KEY_PRESS_MASK|Gdk.EventMask.KEY_RELEASE_MASK)
def pointer(_,e):
    if e.button==1 and e.type in (Gdk.EventType.BUTTON_PRESS,Gdk.EventType.BUTTON_RELEASE):
        down=e.type==Gdk.EventType.BUTTON_PRESS;counts['clicks' if down else 'releases']+=1;counts['left_held']=down
        if down:counts['last_x']=e.x;counts['last_y']=e.y;area.grab_focus()
    return True
def key(_,e):
    if e.hardware_keycode in (37,105):
        down=e.type==Gdk.EventType.KEY_PRESS;counts['ctrl_down' if down else 'ctrl_up']+=1;counts['ctrl_held']=down
    return True
def draw(_,cr):
    cr.set_source_rgb(.08,.32,.29);cr.paint();cr.set_source_rgb(1,1,1);cr.set_font_size(26)
    cr.move_to(36,72);cr.show_text('Pixels isolated remote desktop acceptance');return True
for name in ('button-press-event','button-release-event'):area.connect(name,pointer)
for name in ('key-press-event','key-release-event'):area.connect(name,key)
area.connect('draw',draw);window.add(area);window.fullscreen();window.show_all();window.present();area.grab_focus()
def report():
    data={**counts,'updated':time.time(),'width':area.get_allocated_width(),'height':area.get_allocated_height(),'focused':window.is_active()}
    temporary=root/'fixture-next.json';temporary.write_text(json.dumps(data));temporary.replace(root/'fixture.json');return True
GLib.timeout_add(100,report)
GLib.timeout_add(300000,lambda:(window.destroy(),Gtk.main_quit(),False)[-1])
window.connect('destroy',lambda _:Gtk.main_quit());Gtk.main()
