using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
class Harness {
    static string fixture,root;
    static Dictionary<string,string> env;
    static readonly List<object> cases=new List<object>();
    static string activeCase;
    static Stopwatch activeTimer;
    static object lastObservation;
    sealed class AssertionFailure : Exception {
        public readonly string Code;
        public AssertionFailure(string code) { Code=code; }
    }
    static void Require(bool ok,string name) { if(!ok) throw new AssertionFailure(name); }
    static string DirectoryFor(string name) { string p=Path.Combine(root,name);Directory.CreateDirectory(p);return p; }
    static int Count(string p) { return Directory.GetFiles(p,"*.started").Length; }
    static void CheckCounts(string p) {
        Require(Count(p)==3,"expected three actual process markers");
        var pids=Directory.GetFiles(p,"*.started").Select(f=>Int32.Parse(File.ReadAllText(f))).ToArray();
        Require(pids.All(pid=>pid>0) && pids.Distinct().Count()==3,"three distinct executed process identities");
    }
    static XenOwnedProcess.Result Run(string dir,string mode,int seconds) {
        return XenOwnedProcess.Run(fixture,new[]{"root",mode,dir},dir,seconds,Path.Combine(dir,"stdout.log"),Path.Combine(dir,"stderr.log"),env);
    }
    static void Case(string name,string mode,int seconds,bool expectedSuccess,bool timeout,bool over) {
        activeCase=name;activeTimer=Stopwatch.StartNew();lastObservation=null;string p=DirectoryFor(name);var r=Run(p,mode,seconds);
        lastObservation=new {name,elapsedMilliseconds=activeTimer.ElapsedMilliseconds,actualExecutions=Count(p),r.treeEmpty,r.timedOut,r.outputExceeded,r.captureFailed,exitCode=r.exitCode,succeeded=r.Succeeded};
        CheckCounts(p);
        long retained=new FileInfo(Path.Combine(p,"stdout.log")).Length+new FileInfo(Path.Combine(p,"stderr.log")).Length;
        Require(r.treeEmpty && !r.captureFailed && r.exitCode.HasValue,"native cleanup and final result");
        Require(r.Succeeded==expectedSuccess && r.timedOut==timeout && r.outputExceeded==over,"native result contract");
        Require(retained<=1024*1024,"combined retained bytes exceed cap");
        cases.Add(new {name,actualExecutions=3,r.treeEmpty,r.timedOut,r.outputExceeded,r.captureFailed,exitCode=r.exitCode,retainedBytes=retained,passed=true});
    }
    static void Rejected(string name,Action<string> action) {
        activeCase=name;activeTimer=Stopwatch.StartNew();lastObservation=null;string p=DirectoryFor(name);bool rejected=false;
        try {action(p);} catch(ArgumentException) {rejected=true;}
        Require(rejected && Count(p)==0,"invalid input must fail before execution");
        cases.Add(new{name,actualExecutions=0,expectedRejection=true,passed=true});
    }
    static int Main(string[] args) {
        if(args.Length!=2 || !OperatingSystem.IsWindows()) return 64;
        fixture=Path.GetFullPath(args[0]);root=Path.GetFullPath(args[1]);
        if(!File.Exists(fixture) || Directory.Exists(root)) return 64;
        Directory.CreateDirectory(root);
        env=new Dictionary<string,string>(StringComparer.OrdinalIgnoreCase);
        foreach(var key in new[]{"SYSTEMROOT","WINDIR","PATH","TEMP","TMP","USERPROFILE","LOCALAPPDATA"}) {
            var value=Environment.GetEnvironmentVariable(key);if(value!=null) env.Add(key,value);
        }
        bool passed=false;string failureType=null,failureCode=null;int? nativeErrorCode=null;
        try {
            Case("normal","normal",10,true,false,false);
            Case("timeout","timeout",3,false,true,false);
            Case("output-budget","overflow",10,false,false,true);
            Case("parent-exit","parent-exit",3,false,true,false);
            Rejected("invalid-env",p=>{var bad=new Dictionary<string,string>(env);bad.Add("UNAPPROVED_FIXTURE_KEY","synthetic");XenOwnedProcess.Run(fixture,new[]{"root","normal",p},p,3,Path.Combine(p,"out"),Path.Combine(p,"err"),bad);});
            Rejected("null-args",p=>XenOwnedProcess.Run(fixture,null,p,3,Path.Combine(p,"out"),Path.Combine(p,"err"),env));
            Rejected("nul-arg",p=>XenOwnedProcess.Run(fixture,new[]{"root\0other"},p,3,Path.Combine(p,"out"),Path.Combine(p,"err"),env));
            Rejected("same-logs",p=>XenOwnedProcess.Run(fixture,new[]{"root","normal",p},p,3,Path.Combine(p,"log"),Path.Combine(p,"log"),env));
            activeCase="concurrent";activeTimer=Stopwatch.StartNew();lastObservation=null;
            string active=DirectoryFor("concurrent-active"),denied=DirectoryFor("concurrent-denied");
            var task=Task.Run(()=>Run(active,"timeout",10));
            var timer=Stopwatch.StartNew();while(Count(active)<3 && !task.IsCompleted && timer.ElapsedMilliseconds<5000) Thread.Sleep(10);
            bool busy=false;
            try {Require(Count(active)==3,"concurrent active fixture readiness");try {Run(denied,"normal",3);} catch(InvalidOperationException) {busy=true;} }
            finally {task.GetAwaiter().GetResult();}
            var concurrent=task.Result;CheckCounts(active);
            Require(busy && Count(denied)==0 && concurrent.treeEmpty && concurrent.timedOut && !concurrent.Succeeded,"single-run admission");
            cases.Add(new{name="concurrent",actualExecutions=3,deniedExecutions=0,concurrent.treeEmpty,passed=true});
            activeCase="injected-cleanup-uncertainty";activeTimer=Stopwatch.StartNew();lastObservation=null;
            string fault=DirectoryFor("injected-cleanup-uncertainty"),after=DirectoryFor("poisoned-next");bool injected=false,blocked=false;
            XenOwnedProcessFaultFixture.InjectCleanupUncertainty=true;
            try {XenOwnedProcessFaultFixture.Run(fixture,new[]{"root","normal",fault},fault,10,Path.Combine(fault,"out"),Path.Combine(fault,"err"),env);} catch(InvalidOperationException) {injected=true;}
            CheckCounts(fault);
            try {XenOwnedProcessFaultFixture.Run(fixture,new[]{"root","normal",after},after,3,Path.Combine(after,"out"),Path.Combine(after,"err"),env);} catch(Exception) {blocked=true;}
            Require(injected && XenOwnedProcessFaultFixture.CleanupObservedEmpty && !XenOwnedProcessFaultFixture.SafeToStart && blocked && Count(after)==0,"injected uncertainty poison");
            cases.Add(new{name="injected-cleanup-uncertainty",actualExecutions=3,deniedExecutions=0,treeEmpty=XenOwnedProcessFaultFixture.CleanupObservedEmpty,injectedAfterRealEmpty=true,passed=true});
            Require(cases.Count==10,"required cases executed");passed=true;
        } catch(Exception e) { failureType=e.GetType().Name;failureCode=(e as AssertionFailure)?.Code;nativeErrorCode=(e as System.ComponentModel.Win32Exception)?.NativeErrorCode; }
        File.WriteAllText(Path.Combine(root,"native-receipt.json"),JsonSerializer.Serialize(new{schema=1,passed,failureType,failureCode,nativeErrorCode,activeCase,elapsedMilliseconds=activeTimer?.ElapsedMilliseconds,lastObservation,cases,productExecuted=false,nativeFixtureExecuted=true,cleanupFailureIsInjected=true}));
        return passed?0:1;
    }
}
