using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json;
class CompilePolicyHarness {
    static readonly string[] arguments={"+1.99.1","test","--frozen","--offline","--bin","xenterm","--target","x86_64-pc-windows-msvc","--jobs","2","--no-run","--message-format=json"};
    static readonly List<object> cases=new List<object>();
    static string active="initial",root,fixture;
    static Dictionary<string,string> env;
    static void Need(bool value,string code) { if(!value) throw new InvalidOperationException(code); }
    static string DirectoryFor(string name,string mode) {
        var path=Path.Combine(root,name);Directory.CreateDirectory(path);
        File.WriteAllText(Path.Combine(path,"fixture-mode.txt"),mode);return path;
    }
    static int Markers(string path) {
        var files=Directory.GetFiles(path,"*.started");
        var pids=files.Select(f=>Int32.Parse(File.ReadAllText(f))).ToArray();
        Need(pids.All(p=>p>0) && pids.Distinct().Count()==pids.Length,"marker-identities");return pids.Length;
    }
    static void Case(string name,string mode,bool injected) {
        active=name;var path=DirectoryFor(name,mode);string stdout=Path.Combine(path,"stdout.log"),stderr=Path.Combine(path,"stderr.log");
        // Both APIs receive the unchanged production value 2700. Only the
        // separately generated ShortClock class substitutes its internal clock threshold.
        if(injected) {
            var r=ShortClockOwnedProcess.Run(fixture,arguments,path,2700,stdout,stderr,env,"natural-tree","cargo-compile-only-v1");
            Need(ShortClockOwnedProcess.SafeToStart && r.cleanupMilliseconds<=5000,"injected-cleanup-budget");
            Need(Markers(path)==3 && r.timedOut && !r.Succeeded && r.treeEmpty && !r.captureFailed && r.exitCode.HasValue,"injected-timeout-cleanup");
            cases.Add(new{name,passed=true,actualExecutions=3,injectedClock=true,injectedMilliseconds=8000,productionRequestedSeconds=2700,r.treeEmpty,r.timedOut,r.cleanupMilliseconds,safeToStart=ShortClockOwnedProcess.SafeToStart});return;
        }
        var result=XenOwnedProcess.Run(fixture,arguments,path,2700,stdout,stderr,env,"natural-tree","cargo-compile-only-v1");
        long stdoutBytes=new FileInfo(stdout).Length,stderrBytes=new FileInfo(stderr).Length;
        long retained=stdoutBytes+stderrBytes;
        Need(XenOwnedProcess.SafeToStart && result.cleanupMilliseconds<=5000,"production-cleanup-budget");
        Need(Markers(path)==3 && result.treeEmpty && !result.captureFailed && result.exitCode.HasValue && !result.timedOut,"production-policy-cleanup");
        Need(result.outputBudget==16L*1024*1024 && retained<=result.outputBudget,"hard-cap");
        if(mode=="normal") Need(result.Succeeded && result.exitCode==0 && result.boundaryActiveProcessCount==0 && !result.forcedOwnedCleanup,"natural-tree-success");
        else Need(result.outputExceeded && !result.Succeeded && retained==result.outputBudget && stdoutBytes>0 && stderrBytes>0,"real-overflow-hard-cap");
        cases.Add(new{name,passed=true,actualExecutions=3,injectedClock=false,productionRequestedSeconds=2700,retainedBytes=retained,stdoutBytes,stderrBytes,result.cleanupMilliseconds,safeToStart=XenOwnedProcess.SafeToStart,result.treeEmpty,result.outputExceeded,result.forcedOwnedCleanup});
    }
    static void Reject(string name,string[] args,int seconds,string mode,string policy) {
        active=name;var path=DirectoryFor(name,"normal");bool rejected=false;
        try { XenOwnedProcess.Run(fixture,args,path,seconds,Path.Combine(path,"stdout.log"),Path.Combine(path,"stderr.log"),env,mode,policy); }
        catch(ArgumentException) {rejected=true;}
        Need(rejected && Markers(path)==0,"invalid-command-executed");
        cases.Add(new{name,passed=true,actualExecutions=0,expectedRejection=true});
    }
    static int Main(string[] args) {
        if(args.Length!=2 || !OperatingSystem.IsWindows()) return 64;
        fixture=Path.GetFullPath(args[0]);root=Path.GetFullPath(args[1]);
        if(!File.Exists(fixture) || Path.GetFileName(fixture)!="cargo.exe" || Directory.Exists(root)) return 64;
        Directory.CreateDirectory(root);env=new Dictionary<string,string>(StringComparer.OrdinalIgnoreCase);
        foreach(string key in new[]{"SYSTEMROOT","WINDIR","PATH"}) {var value=Environment.GetEnvironmentVariable(key);if(value!=null) env[key]=value;}
        foreach(string key in new[]{"HOME","USERPROFILE","TEMP","TMP","APPDATA","LOCALAPPDATA"}) env[key]=root;
        bool passed=false;string failureCase=null;
        try {
            Case("compile-policy-natural-tree-cleanup","normal",false);
            Case("compile-policy-output-hard-cap","overflow",false);
            Case("compile-policy-timeout-cleanup","timeout",true);
            Reject("compile-policy-argv-rejection",arguments.Where(a=>a!="--no-run").ToArray(),2700,"natural-tree","cargo-compile-only-v1");
            Reject("compile-policy-root-mode-rejection",arguments,2700,"root-completed-then-owned-cleanup","cargo-compile-only-v1");
            Reject("compile-policy-deadline-rejection",arguments,2701,"natural-tree","cargo-compile-only-v1");
            Reject("unknown-policy-rejection",arguments,2700,"natural-tree","unknown");
            Need(cases.Count==7,"case-count");passed=true;
        } catch {failureCase=active;}
        using(var file=new FileStream(Path.Combine(root,"native-result.json"),FileMode.CreateNew,FileAccess.Write))
            JsonSerializer.Serialize(file,new{passed,case_count=cases.Count,cases,failureCase,productCompileAttempted=false,productRuntimeAttempted=false,full45MinuteElapsedValidation=false});
        return passed?0:1;
    }
}
