// Draft only: fixed local UI gestures, no activation, text entry, or connection command.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Threading;
namespace XenTermNativeProbe {
    public sealed class LimitedInput : IDisposable {
        [StructLayout(LayoutKind.Sequential)] struct POINT { public int X,Y; }
        [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT { public int dx,dy; public uint mouseData,dwFlags,time; public UIntPtr dwExtraInfo; }
        [StructLayout(LayoutKind.Sequential)] struct KEYBDINPUT { public ushort wVk,wScan; public uint dwFlags,time; public UIntPtr dwExtraInfo; }
        [StructLayout(LayoutKind.Explicit)] struct UNION { [FieldOffset(0)] public MOUSEINPUT mouse; [FieldOffset(0)] public KEYBDINPUT key; }
        [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public UNION data; }
        [DllImport("user32.dll",SetLastError=true)] static extern uint SendInput(uint count,INPUT[] inputs,int size);
        [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
        [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd,out uint pid);
        [DllImport("user32.dll")] static extern short GetAsyncKeyState(int key);
        [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
        [DllImport("user32.dll")] static extern bool GetCursorPos(out POINT point);
        [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT point);
        [DllImport("user32.dll")] static extern bool IsChild(IntPtr parent,IntPtr child);
        readonly IntPtr hwnd; readonly int pid; readonly Rectangle visible,desktop;
        readonly Stopwatch timer=Stopwatch.StartNew(); readonly List<ushort> keys=new List<ushort>();
        bool leftDown,disposed; public int AcceptedEvents { get; private set; }
        public bool CleanupConfirmed { get; private set; }
        public string CleanupError { get; private set; }
        public LimitedInput(IntPtr target,int expectedPid,Rectangle visibleBounds,Rectangle desktopBounds) {
            if (IntPtr.Size!=8 || Marshal.SizeOf(typeof(INPUT))!=40) throw new Exception("Unexpected x64 INPUT layout");
            hwnd=target; pid=expectedPid; visible=visibleBounds; desktop=desktopBounds;
            if (GetSystemMetrics(23)!=0) throw new Exception("Swapped mouse mapping is outside the reviewed layout");
            AssertForeground(); AssertReleased();
        }
        public static void RequireForeground(IntPtr target,int expectedPid) {
            uint actual;
            if (GetForegroundWindow()!=target || GetWindowThreadProcessId(target,out actual)==0 || actual!=expectedPid) throw new Exception("Foreground HWND/PID mismatch; no focus stealing");
            using(var own=Process.GetCurrentProcess()) using(var app=Process.GetProcessById(expectedPid))
                if(app.SessionId!=own.SessionId || app.HasExited) throw new Exception("App session/liveness mismatch");
        }
        public void AssertForeground() {
            RequireForeground(hwnd,pid);
            if (timer.Elapsed.TotalSeconds>25) throw new Exception("25-second input budget expired");
        }
        public void AssertReleased() {
            foreach(int key in new int[]{1,2,4,5,6,16,17,18,27,0x4b,0x5b,0x5c})
                if(GetAsyncKeyState(key)<0) throw new Exception("Relevant key/button already held; leave it untouched");
        }
        void Send(INPUT value) {
            AssertForeground();
            if(AcceptedEvents>=13) throw new Exception("Input event budget exceeded");
            uint count=SendInput(1,new INPUT[]{value},Marshal.SizeOf(typeof(INPUT)));
            if(count!=1) throw new Exception("SendInput short write: "+count+"; Win32="+Marshal.GetLastWin32Error()+" (cause including UIPI is unproven)");
            AcceptedEvents++;
        }
        static INPUT Mouse(uint flags,int x,int y) { return new INPUT { type=0,data=new UNION { mouse=new MOUSEINPUT { dx=x,dy=y,dwFlags=flags } } }; }
        static INPUT Key(ushort key,bool up) { return new INPUT { type=1,data=new UNION { key=new KEYBDINPUT { wVk=key,dwFlags=up?2u:0u } } }; }
        void PointOwned(int x,int y) {
            if(!visible.Contains(x,y)) throw new Exception("Input point outside verified visible client/work area");
            var actual=WindowFromPoint(new POINT {X=x,Y=y});
            if(actual!=hwnd && !IsChild(hwnd,actual)) throw new Exception("Input point does not belong to target window");
        }
        void Move(int x,int y) {
            PointOwned(x,y);
            int nx=(int)Math.Round((x-desktop.Left)*65535.0/(desktop.Width-1));
            int ny=(int)Math.Round((y-desktop.Top)*65535.0/(desktop.Height-1));
            Send(Mouse(0xC001,nx,ny)); // MOVE | ABSOLUTE | VIRTUALDESK
            POINT actual; if(!GetCursorPos(out actual) || Math.Abs(actual.X-x)>1 || Math.Abs(actual.Y-y)>1) throw new Exception("Cursor did not reach the verified target point");
            PointOwned(x,y);
        }
        void Down() { Send(Mouse(2,0,0)); leftDown=true; }
        void Up() { Send(Mouse(4,0,0)); leftDown=false; }
        void KeyDown(ushort key) { Send(Key(key,false)); keys.Add(key); }
        void KeyUp(ushort key) { Send(Key(key,true)); keys.Remove(key); }
        public void Hover(int x,int y) { AssertReleased(); Move(x,y); }
        // Read-only phase evidence; this sends no input and never changes focus.
        public object DiagnosticSnapshot(int targetX,int targetY) {
            AssertForeground();
            POINT actual; uint foregroundPid;
            if(!GetCursorPos(out actual)) throw new Exception("Cannot read diagnostic cursor");
            var foreground=GetForegroundWindow();
            if(GetWindowThreadProcessId(foreground,out foregroundPid)==0) throw new Exception("Cannot read foreground PID");
            return new { elapsedMilliseconds=timer.ElapsedMilliseconds, targetX=targetX, targetY=targetY,
                actualX=actual.X, actualY=actual.Y, foregroundHwnd=foreground.ToInt64(), foregroundPid=foregroundPid,
                acceptedEvents=AcceptedEvents, ownedLeftDown=leftDown, asyncLeftDown=(GetAsyncKeyState(1)&0x8000)!=0 };
        }
        public void PressDiagnostic(int x,int y) {
            AssertReleased(); PointOwned(x,y);
            POINT actual;
            if(!GetCursorPos(out actual) || Math.Abs(actual.X-x)>1 || Math.Abs(actual.Y-y)>1) throw new Exception("Diagnostic cursor moved before Down");
            Down();
        }
        public bool DiagnosticLeftHeld() { AssertForeground(); return leftDown && (GetAsyncKeyState(1) & 0x8000)!=0; }
        public void ReleaseDiagnostic() {
            AssertForeground();
            if(!leftDown) throw new Exception("No owned diagnostic press to release");
            Up();
        }
        public void Click(int x,int y) { AssertReleased(); Move(x,y); Down(); Up(); }
        public void Drag(int startX,int y,int endX) {
            AssertReleased(); Move(startX,y); Down();
            for(int n=1;n<=8;n++) { Thread.Sleep(45); Move(startX+(endX-startX)*n/8,y); }
            Up();
        }
        public void QuickConnect() { AssertReleased(); KeyDown(17); KeyDown(0x4b); KeyUp(0x4b); KeyUp(17); }
        public void Escape() { AssertReleased(); KeyDown(27); KeyUp(27); }
        public void Dispose() {
            if(disposed) return; disposed=true; CleanupConfirmed=true;
            // Only releases for successfully accepted downs owned by this instance.
            // A lost foreground still requires these narrow releases, never a fresh down.
            var releases=new List<INPUT>();
            if(leftDown) releases.Add(Mouse(4,0,0));
            for(int n=keys.Count-1;n>=0;n--) releases.Add(Key(keys[n],true));
            foreach(var release in releases) {
                uint accepted=SendInput(1,new INPUT[]{release},Marshal.SizeOf(typeof(INPUT)));
                if(accepted!=1) { CleanupConfirmed=false; CleanupError="Owned input release failed; no retry or elevation"; }
                else AcceptedEvents++;
            }
            leftDown=false; keys.Clear();
        }
    }
    public static class LimitedPixels {
        static bool Near(Color a,Color b,int tolerance) { return Math.Abs(a.R-b.R)<=tolerance && Math.Abs(a.G-b.G)<=tolerance && Math.Abs(a.B-b.B)<=tolerance; }
        public static double Match(Bitmap actual,Bitmap reference,Rectangle area) {
            int good=0,total=0;
            for(int y=area.Top;y<area.Bottom;y+=2) for(int x=area.Left;x<area.Right;x+=2) { total++; if(Near(actual.GetPixel(x,y),reference.GetPixel(x,y),24)) good++; }
            return good/(double)total;
        }
        public static double Changed(Bitmap before,Bitmap after,Rectangle area) { return 1.0-Match(after,before,area); }
        public static double Dark(Bitmap image,Rectangle area) {
            int good=0,total=0; for(int y=area.Top;y<area.Bottom;y+=2) for(int x=area.Left;x<area.Right;x+=2) { var c=image.GetPixel(x,y); total++; if(c.R<110 && c.G<110 && c.B<110) good++; } return good/(double)total;
        }
        public static double Light(Bitmap image,Rectangle area) {
            int good=0,total=0; for(int y=area.Top;y<area.Bottom;y+=2) for(int x=area.Left;x<area.Right;x+=2) { var c=image.GetPixel(x,y); total++; if(c.R>235 && c.G>235 && c.B>235) good++; } return good/(double)total;
        }
        public static int OuterEdge(Bitmap image,int expected) {
            for(int x=expected-2;x<=expected+2;x++) {
                int hits=0,total=0;
                for(int y=400;y<650;y+=5) { var edge=image.GetPixel(x,y); var right=image.GetPixel(x+2,y); total++;
                    if(Near(edge,Color.FromArgb(229,229,229),8) && right.R>248 && right.G>248 && right.B>248) hits++; }
                if(hits>=total*0.94) return x;
            }
            throw new Exception("Visible sidebar outer edge missing at expected width");
        }
        public static int[] InnerEdges(Bitmap image,int outer) {
            // Two static section dividers, never dynamic resource values or curves.
            var ends=new List<int>();
            foreach(int expectedY in new int[]{73,174}) {
                int found=-1;
                for(int y=expectedY-1;y<=expectedY+1;y++) {
                    int left=0,right=0;
                    for(int x=60;x<outer-2;x++) {
                        bool line=Near(image.GetPixel(x,y),Color.FromArgb(229,229,229),5)
                            && Near(image.GetPixel(x,y-1),Color.FromArgb(250,250,250),4)
                            && Near(image.GetPixel(x,y+1),Color.FromArgb(250,250,250),4);
                        if(line) { if(left==0) left=x; right=x; }
                        else if(left!=0) break;
                    }
                    if(Math.Abs(left-65)<=1 && Math.Abs(right-(outer-8))<=2) {
                        if(found!=-1) throw new Exception("Ambiguous inner section divider");
                        found=right;
                    }
                }
                if(found<0) throw new Exception("Inner divider did not follow outer sidebar at row "+expectedY);
                ends.Add(found);
            }
            return ends.ToArray();
        }
    }
}
