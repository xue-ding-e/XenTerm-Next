// Draft Windows-native probe. Only framework assemblies and Windows system DLLs.
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;

namespace XenTermNativeProbe {
    public static class Desktop {
        [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
        [StructLayout(LayoutKind.Sequential)] public struct Point { public int X, Y; }
        [DllImport("user32.dll", SetLastError=true)] static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
        [DllImport("user32.dll", SetLastError=true)] static extern bool CloseDesktop(IntPtr desktop);
        [DllImport("user32.dll")] static extern IntPtr GetThreadDesktop(uint thread);
        [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
        [DllImport("user32.dll")] static extern IntPtr GetProcessWindowStation();
        [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool GetUserObjectInformation(IntPtr obj, int index, IntPtr buffer, uint length, out uint needed);
        [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
        [DllImport("wtsapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool WTSQuerySessionInformation(IntPtr server, int session, int info, out IntPtr buffer, out int bytes);
        [DllImport("wtsapi32.dll")] static extern void WTSFreeMemory(IntPtr memory);
        [DllImport("kernel32.dll")] public static extern uint WTSGetActiveConsoleSessionId();
        [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
        [DllImport("user32.dll")] public static extern IntPtr GetThreadDpiAwarenessContext();
        [DllImport("user32.dll")] public static extern int GetAwarenessFromDpiAwarenessContext(IntPtr context);
        [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
        [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
        [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
        [DllImport("user32.dll", SetLastError=true)] static extern bool GetClientRect(IntPtr hwnd, out Rect rect);
        [DllImport("user32.dll", SetLastError=true)] static extern bool ClientToScreen(IntPtr hwnd, ref Point point);
        [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point point);
        [DllImport("user32.dll")] static extern bool IsChild(IntPtr parent, IntPtr child);
        [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
        [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder text, int size);
        [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr hwnd, int attribute, out int value, int size);
        delegate bool EnumCallback(IntPtr hwnd, IntPtr arg);
        [DllImport("user32.dll")] static extern bool EnumWindows(EnumCallback callback, IntPtr arg);

        static string ObjectName(IntPtr handle) {
            uint needed;
            GetUserObjectInformation(handle, 2, IntPtr.Zero, 0, out needed);
            if (needed == 0 || needed > 4096) throw new Win32Exception(Marshal.GetLastWin32Error());
            IntPtr p = Marshal.AllocHGlobal((int)needed);
            try {
                if (!GetUserObjectInformation(handle, 2, p, needed, out needed)) throw new Win32Exception(Marshal.GetLastWin32Error());
                return Marshal.PtrToStringUni(p);
            } finally { Marshal.FreeHGlobal(p); }
        }
        static int ObjectInt(IntPtr handle, int index) {
            uint needed;
            IntPtr p = Marshal.AllocHGlobal(4);
            try {
                if (!GetUserObjectInformation(handle, index, p, 4, out needed)) throw new Win32Exception(Marshal.GetLastWin32Error());
                return Marshal.ReadInt32(p);
            } finally { Marshal.FreeHGlobal(p); }
        }
        public static Dictionary<string, object> Inspect() {
            var d = new Dictionary<string, object>();
            int session = Process.GetCurrentProcess().SessionId;
            d["user_interactive"] = Environment.UserInteractive;
            d["process_session_id"] = session;
            d["active_console_session_id"] = WTSGetActiveConsoleSessionId();
            IntPtr info; int bytes;
            if (!WTSQuerySessionInformation(IntPtr.Zero, session, 8, out info, out bytes)) throw new Win32Exception(Marshal.GetLastWin32Error(), "WTS connection state query failed");
            try { if (bytes < 4) throw new Exception("Short WTS state response"); d["wts_connect_state"] = Marshal.ReadInt32(info); }
            finally { WTSFreeMemory(info); }
            IntPtr current = GetThreadDesktop(GetCurrentThreadId());
            d["current_desktop"] = ObjectName(current);
            d["current_desktop_receives_input"] = ObjectInt(current, 6) != 0;
            d["window_station"] = ObjectName(GetProcessWindowStation());
            IntPtr input = OpenInputDesktop(0, false, 0x0001); // DESKTOP_READOBJECTS only.
            d["open_input_desktop"] = input != IntPtr.Zero;
            d["open_input_desktop_error"] = input == IntPtr.Zero ? Marshal.GetLastWin32Error() : 0;
            if (input != IntPtr.Zero) {
                try { d["input_desktop"] = ObjectName(input); }
                finally { CloseDesktop(input); }
            }
            d["primary_width"] = GetSystemMetrics(0); d["primary_height"] = GetSystemMetrics(1);
            d["virtual_left"] = GetSystemMetrics(76); d["virtual_top"] = GetSystemMetrics(77);
            d["virtual_width"] = GetSystemMetrics(78); d["virtual_height"] = GetSystemMetrics(79);
            d["dpi_awareness"] = GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext());
            return d;
        }
        public static long[] WindowsForProcess(int processId) {
            var result = new List<long>();
            EnumCallback callback = delegate(IntPtr window, IntPtr ignored) {
                uint pid; GetWindowThreadProcessId(window, out pid);
                if (pid == processId && IsWindowVisible(window) && !IsIconic(window)) result.Add(window.ToInt64());
                return true;
            };
            EnumWindows(callback, IntPtr.Zero);
            GC.KeepAlive(callback);
            return result.ToArray();
        }
        public static Rectangle ClientBounds(IntPtr hwnd) {
            Rect r; Point p = new Point();
            if (!GetClientRect(hwnd, out r) || !ClientToScreen(hwnd, ref p)) throw new Win32Exception(Marshal.GetLastWin32Error());
            return new Rectangle(p.X, p.Y, r.Right-r.Left, r.Bottom-r.Top);
        }
        public static string WindowClass(IntPtr hwnd) {
            var text = new StringBuilder(256); GetClassName(hwnd, text, text.Capacity); return text.ToString();
        }
        public static Bitmap CaptureClient(IntPtr hwnd) {
            if (!IsWindowVisible(hwnd) || IsIconic(hwnd)) throw new Exception("Window not mapped or minimized");
            int cloaked;
            int hr = DwmGetWindowAttribute(hwnd, 14, out cloaked, 4);
            if (hr != 0 || cloaked != 0) throw new Exception("Window DWM state unavailable or cloaked: " + hr + "/" + cloaked);
            Rectangle r = ClientBounds(hwnd);
            Rectangle desktop = new Rectangle(GetSystemMetrics(76), GetSystemMetrics(77), GetSystemMetrics(78), GetSystemMetrics(79));
            if (r.Width < 100 || r.Height < 100 || r.Width > 8192 || r.Height > 8192 || !desktop.Contains(r)) throw new Exception("Client outside desktop or invalid dimensions");
            // Reject sampled occlusion. We do not activate/reposition the app or synthesize input.
            for (int yi=0; yi<5; yi++) for (int xi=0; xi<5; xi++) {
                Point p = new Point { X=r.Left+(xi+1)*r.Width/6, Y=r.Top+(yi+1)*r.Height/6 };
                IntPtr actual = WindowFromPoint(p);
                if (actual != hwnd && !IsChild(hwnd, actual)) throw new Exception("Client is occluded at a sampled point");
            }
            var image = new Bitmap(r.Width, r.Height, PixelFormat.Format32bppArgb);
            try {
                using (var graphics = Graphics.FromImage(image)) graphics.CopyFromScreen(r.Left, r.Top, 0, 0, image.Size, CopyPixelOperation.SourceCopy);
                return image;
            } catch { image.Dispose(); throw; }
        }
        public static Dictionary<string, object> PixelMetrics(Bitmap image) {
            int samples=0, bright=0; var bins = new HashSet<int>();
            for (int y=0; y<image.Height; y+=3) for (int x=0; x<image.Width; x+=3) {
                Color c = image.GetPixel(x,y); samples++;
                if (Math.Max(c.R,Math.Max(c.G,c.B)) > 24) bright++;
                bins.Add(((c.R >> 4) << 8) | ((c.G >> 4) << 4) | (c.B >> 4));
            }
            return new Dictionary<string,object> {
                {"width",image.Width},{"height",image.Height},{"sample_count",samples},
                {"quantized_rgb_bins",bins.Count},{"non_dark_fraction",(double)bright/samples},
                {"nonblank_heuristic", bins.Count >= 16 && (double)bright/samples >= 0.01}
            };
        }
        public static bool ControlPixelsMatch(Bitmap image) {
            Color[] expected = { Color.FromArgb(230,40,50), Color.FromArgb(35,190,70), Color.FromArgb(40,90,230), Color.FromArgb(240,210,40) };
            for (int n=0; n<4; n++) {
                int cx=(n%2==0 ? image.Width/4 : 3*image.Width/4);
                int cy=(n<2 ? image.Height/4 : 3*image.Height/4);
                int matched=0;
                for (int dy=-4;dy<=4;dy++) for (int dx=-4;dx<=4;dx++) {
                    Color actual=image.GetPixel(cx+dx,cy+dy), e=expected[n];
                    if (Math.Abs(actual.R-e.R)<=12 && Math.Abs(actual.G-e.G)<=12 && Math.Abs(actual.B-e.B)<=12) matched++;
                }
                if (matched<75) return false;
            }
            return true;
        }
    }
    public sealed class ControlForm : Form {
        public ControlForm() {
            Text="XenTerm native desktop control (temporary)";
            ClientSize=new Size(640,360); StartPosition=FormStartPosition.Manual;
            Location=new System.Drawing.Point(40,40); TopMost=true; ShowInTaskbar=false;
            AutoScaleMode=AutoScaleMode.None; DoubleBuffered=true;
        }
        protected override void OnPaint(PaintEventArgs e) {
            base.OnPaint(e);
            Color[] colors={Color.FromArgb(230,40,50),Color.FromArgb(35,190,70),Color.FromArgb(40,90,230),Color.FromArgb(240,210,40)};
            for (int i=0;i<4;i++) using (var b=new SolidBrush(colors[i])) e.Graphics.FillRectangle(b,(i%2)*ClientSize.Width/2,(i/2)*ClientSize.Height/2,ClientSize.Width/2,ClientSize.Height/2);
            using(var font=new Font("Segoe UI",18,FontStyle.Bold)) e.Graphics.DrawString("NATIVE DESKTOP CONTROL",font,Brushes.White,12,12);
        }
    }
    // Create suspended, attach to a private kill-on-close job, then resume. No child can
    // spawn outside containment before assignment. Timeout and cleanup only affect our job.
    public sealed class OwnedProcess : IDisposable {
        [StructLayout(LayoutKind.Sequential)] struct STARTUPINFO { public int cb; public IntPtr reserved, desktop, title; public int x,y,cx,cy,xc,yc,fill,flags; public short show,reserved2; public IntPtr reservedPtr,input,output,error; }
        [StructLayout(LayoutKind.Sequential)] struct PROCESS_INFORMATION { public IntPtr process,thread; public uint pid,tid; }
        [StructLayout(LayoutKind.Sequential)] struct BASIC_LIMIT { public long processTime,jobTime; public uint flags; public UIntPtr minWorking,maxWorking; public uint activeLimit; public UIntPtr affinity; public uint priority,scheduling; }
        [StructLayout(LayoutKind.Sequential)] struct IO_COUNTERS { public ulong readOps,writeOps,otherOps,readBytes,writeBytes,otherBytes; }
        [StructLayout(LayoutKind.Sequential)] struct EXTENDED_LIMIT { public BASIC_LIMIT basic; public IO_COUNTERS io; public UIntPtr processMemory,jobMemory,peakProcess,peakJob; }
        [StructLayout(LayoutKind.Sequential)] struct ACCOUNTING { public long userTime,kernelTime,periodUser,periodKernel; public uint faults,totalProcesses,activeProcesses,terminatedProcesses; }
        [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr CreateJobObject(IntPtr attrs,string name);
        [DllImport("kernel32.dll",SetLastError=true)] static extern bool SetInformationJobObject(IntPtr job,int info,ref EXTENDED_LIMIT value,int size);
        [DllImport("kernel32.dll",SetLastError=true)] static extern bool QueryInformationJobObject(IntPtr job,int info,out ACCOUNTING value,int size,IntPtr needed);
        [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool CreateProcess(string app,StringBuilder command,IntPtr processAttrs,IntPtr threadAttrs,bool inherit,uint flags,IntPtr environment,string cwd,ref STARTUPINFO startup,out PROCESS_INFORMATION info);
        [DllImport("kernel32.dll",SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr job,IntPtr process);
        [DllImport("kernel32.dll",SetLastError=true)] static extern uint ResumeThread(IntPtr thread);
        [DllImport("kernel32.dll",SetLastError=true)] static extern bool TerminateProcess(IntPtr process,uint code);
        [DllImport("kernel32.dll",SetLastError=true)] static extern bool TerminateJobObject(IntPtr job,uint code);
        [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr handle,uint milliseconds);
        [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetExitCodeProcess(IntPtr process,out uint code);
        [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
        IntPtr job,process; System.Threading.Timer timer; public int Pid {get;private set;}
        public bool TimedOut {get;private set;}
        public bool CleanupConfirmed {get;private set;}
        public OwnedProcess(string executable,string arguments,string cwd,int timeoutSeconds) {
            if (timeoutSeconds<1 || timeoutSeconds>3600) throw new ArgumentException("Invalid timeout");
            job=CreateJobObject(IntPtr.Zero,null);
            if(job==IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
            var limits=new EXTENDED_LIMIT(); limits.basic.flags=0x2000; // KILL_ON_JOB_CLOSE
            if(!SetInformationJobObject(job,9,ref limits,Marshal.SizeOf(typeof(EXTENDED_LIMIT)))) { int e=Marshal.GetLastWin32Error(); CloseHandle(job); job=IntPtr.Zero; throw new Win32Exception(e); }
            var startup=new STARTUPINFO(); startup.cb=Marshal.SizeOf(typeof(STARTUPINFO));
            PROCESS_INFORMATION pi;
            if(!CreateProcess(executable,new StringBuilder("\""+executable+"\" "+arguments),IntPtr.Zero,IntPtr.Zero,false,0x08000004,IntPtr.Zero,cwd,ref startup,out pi)) { int e=Marshal.GetLastWin32Error(); Dispose(); throw new Win32Exception(e); }
            process=pi.process; Pid=(int)pi.pid;
            try {
                if(!AssignProcessToJobObject(job,process)) { int e=Marshal.GetLastWin32Error(); TerminateProcess(process,125); throw new Win32Exception(e,"Private job assignment failed; child never resumed"); }
                timer=new System.Threading.Timer(delegate(object _) { TimedOut=true; TerminateJobObject(job,124); },null,timeoutSeconds*1000,Timeout.Infinite);
                if(ResumeThread(pi.thread)==0xffffffff) throw new Win32Exception(Marshal.GetLastWin32Error());
            } catch { Dispose(); throw; } finally { CloseHandle(pi.thread); }
        }
        public bool HasExited { get { return WaitForSingleObject(process,0)==0; } }
        public int ExitCode { get { uint code; if(!GetExitCodeProcess(process,out code)) throw new Win32Exception(Marshal.GetLastWin32Error()); return unchecked((int)code); } }
        public bool Wait(int milliseconds) { return WaitForSingleObject(process,(uint)milliseconds)==0; }
        public void Dispose() {
            if(timer!=null) { using(var finished=new ManualResetEvent(false)) { if(timer.Dispose(finished)) finished.WaitOne(5000); } timer=null; }
            if(job!=IntPtr.Zero) {
                TerminateJobObject(job,125);
                var watch=Stopwatch.StartNew();
                do {
                    ACCOUNTING a;
                    if(QueryInformationJobObject(job,1,out a,Marshal.SizeOf(typeof(ACCOUNTING)),IntPtr.Zero) && a.activeProcesses==0) { CleanupConfirmed=true; break; }
                    Thread.Sleep(50);
                } while(watch.ElapsedMilliseconds<5000);
                CloseHandle(job); job=IntPtr.Zero;
            }
            if(process!=IntPtr.Zero) { CloseHandle(process); process=IntPtr.Zero; }
        }
    }
}
