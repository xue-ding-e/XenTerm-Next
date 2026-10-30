using System;
using System.IO;
using System.Text;
using System.Diagnostics;
using System.ComponentModel;
using System.Collections.Generic;
using System.Security.Cryptography;
using System.Runtime.InteropServices;
using System.Threading;
using Microsoft.Win32.SafeHandles;
public static class XenOwnedProcess {
    private static readonly object runGate=new object();
    private static bool safeToStart=true;
    public static bool SafeToStart { get { lock(runGate) return safeToStart; } }
    [StructLayout(LayoutKind.Sequential)] struct SA { public int length; public IntPtr descriptor; public int inherit; }
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] struct SI {
        public int cb; public string reserved, desktop, title; public uint x,y,xsize,ysize,xchars,ychars,fill,flags;
        public short show, reserved2; public IntPtr reserved3, input, output, error;
    }
    [StructLayout(LayoutKind.Sequential)] struct PI { public IntPtr process, thread; public uint pid, tid; }
    [StructLayout(LayoutKind.Sequential)] struct BASIC {
        public long processTime, jobTime; public uint flags; public UIntPtr min, max; public uint activeLimit;
        public UIntPtr affinity; public uint priority, scheduling;
    }
    [StructLayout(LayoutKind.Sequential)] struct IO { public ulong ro,wo,oo,rt,wt,ot; }
    [StructLayout(LayoutKind.Sequential)] struct EXTENDED { public BASIC basic; public IO io; public UIntPtr processMemory,jobMemory,peakProcess,peakJob; }
    [StructLayout(LayoutKind.Sequential)] struct ACCOUNTING { public long user,kernel,periodUser,periodKernel; public uint faults,total,active,terminated; }
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr CreateJobObject(IntPtr a, string name);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetInformationJobObject(IntPtr job,int kind,ref EXTENDED info,uint size);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool QueryInformationJobObject(IntPtr job,int kind,out ACCOUNTING info,uint size,IntPtr returned);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr job,IntPtr process);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool TerminateJobObject(IntPtr job,uint code);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool TerminateProcess(IntPtr process,uint code);
    [DllImport("kernel32.dll", SetLastError=true)] static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError=true)] static extern uint WaitForSingleObject(IntPtr handle,uint milliseconds);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetExitCodeProcess(IntPtr process,out uint code);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr CreateFile(string path,uint access,uint share,ref SA security,uint creation,uint flags,IntPtr template);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool CreateProcess(string app,StringBuilder command,IntPtr processSecurity,IntPtr threadSecurity,bool inherit,uint flags,IntPtr environment,string directory,ref SI start,out PI process);
    [StructLayout(LayoutKind.Sequential)] struct SIX { public SI start; public IntPtr attributes; }
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool InitializeProcThreadAttributeList(IntPtr list,int count,uint flags,ref IntPtr size);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool UpdateProcThreadAttribute(IntPtr list,uint flags,IntPtr attribute,IntPtr value,IntPtr size,IntPtr previous,IntPtr returned);
    [DllImport("kernel32.dll")] static extern void DeleteProcThreadAttributeList(IntPtr list);
    [DllImport("kernel32.dll", EntryPoint="CreateProcessW", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool CreateProcessExact(string app,StringBuilder command,IntPtr processSecurity,IntPtr threadSecurity,bool inherit,uint flags,IntPtr environment,string directory,ref SIX start,out PI process);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool CreatePipe(out IntPtr read,out IntPtr write,ref SA security,uint size);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetHandleInformation(IntPtr handle,uint mask,uint flags);
    // Pipe readers retain at most the explicit shared policy budget on disk; excess bytes are discarded
    // while the owner terminates its job. Readers never print captured content.
    sealed class Capture {
        readonly object gate=new object(); readonly long budget; long written; bool exceeded,failed;
        readonly Thread first,second;readonly Action stopOwnedJob;bool stopEnabled=true;
        public void DisableStop() { lock(gate) stopEnabled=false; }
        public bool Exceeded { get { lock(gate) return exceeded; } }
        public bool Failed { get { lock(gate) return failed; } }
        public Capture(IntPtr stdout,IntPtr stderr,string output,string error,Action stop,long budget) {
            this.budget=budget;
            stopOwnedJob=stop;
            first=new Thread(()=>Drain(stdout,output));second=new Thread(()=>Drain(stderr,error));
            first.IsBackground=true;second.IsBackground=true;first.Start();second.Start();
        }
        void Drain(IntPtr handle,string path) {
            try {
                using(var read=new FileStream(new SafeFileHandle(handle,false),FileAccess.Read,4096,false))
                using(var file=new FileStream(path,FileMode.CreateNew,FileAccess.Write,FileShare.Read)) {
                    var buffer=new byte[4096];int count;
                    while((count=read.Read(buffer,0,buffer.Length))>0) {
                        bool overflow=false;
                        lock(gate) {
                            int keep=(int)Math.Min(count,Math.Max(0,budget-written));
                            if(keep>0) {file.Write(buffer,0,keep);written+=keep;}
                            if(keep<count && !exceeded) {exceeded=true;overflow=true;}
                            if(overflow && stopEnabled) stopOwnedJob();
                        }
                    }
                }
            } catch { lock(gate) failed=true; }
        }
        public bool Join(Stopwatch deadline) { return first.Join(Remaining(deadline)) && second.Join(Remaining(deadline)); }
    }
    static string Quote(string value) {
        if (value.Length>0 && value.IndexOfAny(new[]{' ','\t','\n','\r','\v','"'})<0) return value;
        var result = new StringBuilder("\""); int slashes=0;
        foreach (char c in value) {
            if (c=='\\') { slashes++; continue; }
            if (c=='"') { result.Append('\\',slashes*2+1).Append(c); slashes=0; continue; }
            result.Append('\\',slashes).Append(c); slashes=0;
        }
        return result.Append('\\',slashes*2).Append('"').ToString();
    }

    static void Check(bool value,string stage) { if(!value) throw new Win32Exception(Marshal.GetLastWin32Error(),stage); }
    static uint Active(IntPtr job) { ACCOUNTING a; Check(QueryInformationJobObject(job,1,out a,(uint)Marshal.SizeOf<ACCOUNTING>(),IntPtr.Zero),"query owned job"); return a.active; }
    static int Remaining(Stopwatch deadline) { return (int)Math.Max(0,5000-deadline.ElapsedMilliseconds); }
    static void Empty(IntPtr job,Stopwatch deadline) {
        while(Active(job)!=0) {
            if(Remaining(deadline)==0) throw new Exception("owned cleanup uncertain");
            Thread.Sleep(Math.Min(25,Remaining(deadline)));
        }
    }
    public sealed class Result {
        // Null means no verified final exit code, never a successful default zero.
        public uint? exitCode; public bool timedOut,outputExceeded,treeEmpty,captureFailed;
        public string completionMode,commandPolicy; public long outputBudget,cleanupMilliseconds; public bool forcedOwnedCleanup;
        // Sequential owned-handle observations before cleanup, not an atomic OS snapshot.
        public bool? boundaryRootSignaled; public uint? boundaryActiveProcessCount,boundaryRootExitCode;
        public bool boundaryRootExitCodeKnown { get { return boundaryRootExitCode.HasValue; } }
        public bool Succeeded { get { return exitCode.HasValue && exitCode.Value==0 && treeEmpty && !timedOut && !outputExceeded && !captureFailed; } }
    }
    public static Result Run(string executable,string[] args,string cwd,int seconds,string stdout,string stderr,Dictionary<string,string> environment,string completionMode="natural-tree",string commandPolicy="smoke-v1") {
        if(!Monitor.TryEnter(runGate)) throw new InvalidOperationException("owned run already active");
        try { return RunExclusive(executable,args,cwd,seconds,stdout,stderr,environment,completionMode,commandPolicy); }
        finally { Monitor.Exit(runGate); }
    }
    private static Result RunExclusive(string executable,string[] args,string cwd,int seconds,string stdout,string stderr,Dictionary<string,string> environment,string completionMode,string commandPolicy) {
        if(completionMode!="natural-tree" && completionMode!="root-completed-then-owned-cleanup") throw new ArgumentException("invalid completion mode");
        if(!safeToStart) throw new Exception("previous cleanup uncertain");
        if(args==null || environment==null) throw new ArgumentException("missing explicit arguments or environment");
        foreach(var arg in args) if(arg==null || arg.IndexOf('\0')>=0) throw new ArgumentException("invalid argument");
        if(String.IsNullOrEmpty(cwd) || cwd.IndexOf('\0')>=0 || !Path.IsPathFullyQualified(cwd) || !Directory.Exists(cwd)) throw new ArgumentException("invalid working directory");
        foreach(var path in new[]{executable,stdout,stderr})
            if(String.IsNullOrEmpty(path) || path.IndexOf('\0')>=0 || !Path.IsPathFullyQualified(path)) throw new ArgumentException("invalid absolute path");
        stdout=Path.GetFullPath(stdout);stderr=Path.GetFullPath(stderr);
        if(StringComparer.OrdinalIgnoreCase.Equals(stdout,stderr) || File.Exists(stdout) || File.Exists(stderr) || !Directory.Exists(Path.GetDirectoryName(stdout)) || !Directory.Exists(Path.GetDirectoryName(stderr))) throw new ArgumentException("invalid distinct new log paths");
        bool cargo=commandPolicy=="cargo-compile-only-v1";
        if(commandPolicy!="smoke-v1" && !cargo) throw new ArgumentException("unknown command policy");
        string[] compileArgs={"+1.99.1","test","--frozen","--offline","--bin","xenterm","--target","x86_64-pc-windows-msvc","--jobs","2","--no-run","--message-format=json"};
        if(cargo && (completionMode!="natural-tree" || seconds!=2700 || !String.Equals(Path.GetFileName(executable),"cargo.exe",StringComparison.OrdinalIgnoreCase) || args.Length!=compileArgs.Length)) throw new ArgumentException("compile-only policy");
        if(cargo) for(int i=0;i<args.Length;i++) if(args[i]!=compileArgs[i]) throw new ArgumentException("compile-only arguments");
        long outputBudget=cargo ? 16L*1024*1024 : 1024*1024;
        if(seconds<1 || (!cargo && seconds>30) || !Path.IsPathFullyQualified(executable) || !File.Exists(executable)) throw new ArgumentException("invalid command bound or path");
        var allowed=new HashSet<string>(("PATH SYSTEMROOT WINDIR COMSPEC PATHEXT OS NUMBER_OF_PROCESSORS PROCESSOR_ARCHITECTURE PROCESSOR_IDENTIFIER PROGRAMFILES PROGRAMFILES(X86) PROGRAMW6432 COMMONPROGRAMFILES COMMONPROGRAMFILES(X86) INCLUDE LIB LIBPATH VCINSTALLDIR VCTOOLSINSTALLDIR VCTOOLSVERSION VSINSTALLDIR VISUALSTUDIOVERSION WINDOWSSDKDIR WINDOWSSDKVERSION WINDOWSSDKLIBVERSION UNIVERSALCRTSDKDIR UCRTVERSION VSCMD_ARG_HOST_ARCH VSCMD_ARG_TGT_ARCH CARGO_HOME RUSTUP_HOME LANG LC_ALL TZ HOME USERPROFILE APPDATA LOCALAPPDATA XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME XDG_RUNTIME_DIR XENTERM_DATA_DIR TEMP TMP TMPDIR RUST_BACKTRACE PROGRAMDATA ALLUSERSPROFILE PYTHONUTF8 CARGO_TARGET_DIR").Split(' '),StringComparer.OrdinalIgnoreCase);
        if(cargo) foreach(string key in new[]{"CARGO_NET_OFFLINE","CARGO_BUILD_JOBS","CARGO_INCREMENTAL","CARGO_PROFILE_DEV_DEBUG","CARGO_PROFILE_TEST_DEBUG","XT_VERSION_JSON","GITHUB_SHA","GITHUB_REF_TYPE","GITHUB_REF_NAME"}) allowed.Add(key);
        var envKeys=new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var envLines=new List<string>();
        foreach(var pair in environment) {
            if(!allowed.Contains(pair.Key) || String.IsNullOrEmpty(pair.Key) || pair.Key.IndexOfAny(new[]{'=', '\0'})>=0 || pair.Value==null || pair.Value.IndexOf('\0')>=0 || !envKeys.Add(pair.Key)) throw new ArgumentException("invalid explicit environment");
            envLines.Add(pair.Key+"="+pair.Value);
        }
        envLines.Sort(StringComparer.OrdinalIgnoreCase);
        safeToStart=false;
        IntPtr env=Marshal.StringToHGlobalUni(String.Join("\0",envLines)+"\0\0");
        IntPtr job=IntPtr.Zero,input=IntPtr.Zero,output=IntPtr.Zero,error=IntPtr.Zero,outRead=IntPtr.Zero,errRead=IntPtr.Zero,attributes=IntPtr.Zero,handles=IntPtr.Zero;bool attributesReady=false;Capture capture=null;
        PI p=new PI();bool created=false,assigned=false;var result=new Result {completionMode=completionMode,commandPolicy=commandPolicy,outputBudget=outputBudget};var timer=Stopwatch.StartNew();
        try {
            job=CreateJobObject(IntPtr.Zero,null);Check(job!=IntPtr.Zero,"create job");
            var limit=new EXTENDED();limit.basic.flags=0x2000;
            Check(SetInformationJobObject(job,9,ref limit,(uint)Marshal.SizeOf<EXTENDED>()),"kill-on-close");
            var sa=new SA {length=Marshal.SizeOf<SA>(),inherit=1};
            Check(CreatePipe(out outRead,out output,ref sa,4096),"stdout pipe");
            Check(CreatePipe(out errRead,out error,ref sa,4096),"stderr pipe");
            Check(SetHandleInformation(outRead,1,0) && SetHandleInformation(errRead,1,0),"noninherited readers");
            capture=new Capture(outRead,errRead,stdout,stderr,()=>Check(TerminateJobObject(job,124),"output limit owned termination"),outputBudget);
            input=CreateFile("NUL",0x80000000,3,ref sa,3,0x80,IntPtr.Zero);Check(input!=new IntPtr(-1),"null stdin");
            IntPtr bytes=IntPtr.Zero;InitializeProcThreadAttributeList(IntPtr.Zero,1,0,ref bytes);
            Check(bytes!=IntPtr.Zero,"handle-list size");attributes=Marshal.AllocHGlobal(bytes);
            Check(InitializeProcThreadAttributeList(attributes,1,0,ref bytes),"initialize handle-list");attributesReady=true;
            handles=Marshal.AllocHGlobal(3*IntPtr.Size);Marshal.WriteIntPtr(handles,0,input);Marshal.WriteIntPtr(handles,IntPtr.Size,output);Marshal.WriteIntPtr(handles,2*IntPtr.Size,error);
            Check(UpdateProcThreadAttribute(attributes,0,new IntPtr(0x20002),handles,new IntPtr(3*IntPtr.Size),IntPtr.Zero,IntPtr.Zero),"exact inherited handles");
            var si=new SIX {start=new SI {cb=Marshal.SizeOf<SIX>(),flags=0x100,input=input,output=output,error=error},attributes=attributes};
            var line=new StringBuilder(Quote(executable));foreach(var arg in args) line.Append(' ').Append(Quote(arg));
            Check(CreateProcessExact(executable,line,IntPtr.Zero,IntPtr.Zero,true,0x08080404,env,cwd,ref si,out p),"create suspended");
            created=true;safeToStart=false;
            Check(CloseHandle(output),"close parent stdout writer");output=IntPtr.Zero;
            Check(CloseHandle(error),"close parent stderr writer");error=IntPtr.Zero;
            Check(AssignProcessToJobObject(job,p.process),"assign before execution");assigned=true;
            Check(ResumeThread(p.thread)!=0xffffffff,"resume owned root");
            while(true) {
                uint wait=WaitForSingleObject(p.process,25);Check(wait!=0xffffffff,"wait root");
                result.timedOut=timer.ElapsedMilliseconds>=seconds*1000L;
                result.outputExceeded=capture.Exceeded;result.captureFailed=capture.Failed;
                if(wait==0 && !result.timedOut && !result.outputExceeded && !result.captureFailed && (completionMode=="root-completed-then-owned-cleanup" || Active(job)==0)) break;
                if(result.timedOut || result.outputExceeded || result.captureFailed) break;
            }
            uint boundaryWait=WaitForSingleObject(p.process,0);Check(boundaryWait!=0xffffffff,"boundary root signal");
            result.boundaryRootSignaled=boundaryWait==0;
            result.boundaryActiveProcessCount=Active(job);
            if(boundaryWait==0) {
                uint boundaryCode;Check(GetExitCodeProcess(p.process,out boundaryCode),"boundary root exit code");
                result.boundaryRootExitCode=boundaryCode;
            }
        } finally {
            var cleanupTimer=Stopwatch.StartNew();
            try {
                if(assigned) { if(Active(job)!=0) { result.forcedOwnedCleanup=true;Check(TerminateJobObject(job,124),"terminate owned tree"); }Empty(job,cleanupTimer); }
                else if(created) { Check(TerminateProcess(p.process,125),"terminate suspended root");if(WaitForSingleObject(p.process,(uint)Remaining(cleanupTimer))!=0) throw new Exception("suspended cleanup uncertain"); }
                if(created) {
                    uint remaining=(uint)Remaining(cleanupTimer);
                    uint wait=WaitForSingleObject(p.process,remaining);
                    Check(wait!=0xffffffff,"root exit wait");
                    if(wait!=0) throw new Exception("root exit confirmation uncertain");
                    uint code;Check(GetExitCodeProcess(p.process,out code),"final root exit code");result.exitCode=code;
                }
                if(output!=IntPtr.Zero && output!=new IntPtr(-1)) {Check(CloseHandle(output),"close stdout writer");output=IntPtr.Zero;}
                if(error!=IntPtr.Zero && error!=new IntPtr(-1)) {Check(CloseHandle(error),"close stderr writer");error=IntPtr.Zero;}
                if(capture!=null) {
                    if(!capture.Join(cleanupTimer)) throw new Exception("capture cleanup uncertain");
                    result.outputExceeded=result.outputExceeded || capture.Exceeded;
                    result.captureFailed=result.captureFailed || capture.Failed;
                }
                result.treeEmpty=true;safeToStart=true;
            } finally {
                // Serialize callback disable with overflow termination before releasing its job handle.
                if(capture!=null) capture.DisableStop();
                if(job!=IntPtr.Zero) CloseHandle(job);
                foreach(var h in new[]{p.thread,p.process,input,output,error,outRead,errRead}) if(h!=IntPtr.Zero && h!=new IntPtr(-1)) CloseHandle(h);
                if(attributesReady) DeleteProcThreadAttributeList(attributes);
                if(attributes!=IntPtr.Zero) Marshal.FreeHGlobal(attributes);
                if(handles!=IntPtr.Zero) Marshal.FreeHGlobal(handles);
                Marshal.FreeHGlobal(env);
                result.cleanupMilliseconds=cleanupTimer.ElapsedMilliseconds;
            }
        }
        return result;
    }
}
