using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json;
class FetchPolicyHarness {
    static readonly string[] arguments={"fetch","--locked","--target","x86_64-pc-windows-msvc"};
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
        // Both APIs receive the unchanged production value 600. Only the
        // separately generated ShortClock class substitutes its internal clock threshold.
        if(injected) {
            var r=ShortClockOwnedProcess.Run(fixture,arguments,path,600,stdout,stderr,env,"natural-tree","cargo-fetch-only-v1");
            Need(ShortClockOwnedProcess.SafeToStart && r.cleanupMilliseconds<=5000,"injected-cleanup-budget");
            Need(Markers(path)==3 && r.timedOut && !r.Succeeded && r.treeEmpty && !r.captureFailed && r.exitCode.HasValue,"injected-timeout-cleanup");
            cases.Add(new{name,passed=true,actualExecutions=3,injectedClock=true,injectedMilliseconds=8000,productionRequestedSeconds=600,r.treeEmpty,r.timedOut,r.cleanupMilliseconds,safeToStart=ShortClockOwnedProcess.SafeToStart});return;
        }
        var result=FetchOwnedProcess.Run(fixture,arguments,path,600,stdout,stderr,env,"natural-tree","cargo-fetch-only-v1");
        long stdoutBytes=new FileInfo(stdout).Length,stderrBytes=new FileInfo(stderr).Length;
        long retained=stdoutBytes+stderrBytes;
        Need(FetchOwnedProcess.SafeToStart && result.cleanupMilliseconds<=5000,"production-cleanup-budget");
        Need(Markers(path)==3 && result.treeEmpty && !result.captureFailed && result.exitCode.HasValue && !result.timedOut,"production-policy-cleanup");
        Need(result.outputBudget==1024L*1024 && retained<=result.outputBudget,"hard-cap");
        if(mode=="normal") Need(result.Succeeded && result.exitCode==0 && result.boundaryActiveProcessCount==0 && !result.forcedOwnedCleanup,"natural-tree-success");
        else Need(result.outputExceeded && !result.Succeeded && retained==result.outputBudget && stdoutBytes>0 && stderrBytes>0,"real-overflow-hard-cap");
        cases.Add(new{name,passed=true,actualExecutions=3,injectedClock=false,productionRequestedSeconds=600,retainedBytes=retained,stdoutBytes,stderrBytes,result.cleanupMilliseconds,safeToStart=FetchOwnedProcess.SafeToStart,result.treeEmpty,result.outputExceeded,result.forcedOwnedCleanup});
    }
    static void Reject(string name,string[] args,int seconds,string mode,string policy) {
        active=name;var path=DirectoryFor(name,"normal");bool rejected=false;
        try { FetchOwnedProcess.Run(fixture,args,path,seconds,Path.Combine(path,"stdout.log"),Path.Combine(path,"stderr.log"),env,mode,policy); }
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
            Case("fetch-policy-natural-tree-cleanup","normal",false);
            Case("fetch-policy-output-hard-cap","overflow",false);
            Case("fetch-policy-timeout-cleanup","timeout",true);
            Reject("fetch-policy-argv-rejection",arguments.Concat(new[]{"--offline"}).ToArray(),600,"natural-tree","cargo-fetch-only-v1");
            Reject("fetch-policy-root-mode-rejection",arguments,600,"root-completed-then-owned-cleanup","cargo-fetch-only-v1");
            Reject("fetch-policy-deadline-rejection",arguments,601,"natural-tree","cargo-fetch-only-v1");
            Reject("fetch-policy-rustup-selector-rejection",new[]{"+1.99.1"}.Concat(arguments).ToArray(),600,"natural-tree","cargo-fetch-only-v1");
            Reject("unknown-policy-rejection",arguments,600,"natural-tree","unknown");
            Need(cases.Count==8,"case-count");passed=true;
        } catch {failureCase=active;}
        using(var file=new FileStream(Path.Combine(root,"native-result.json"),FileMode.CreateNew,FileAccess.Write))
            JsonSerializer.Serialize(file,new{passed,case_count=cases.Count,cases,failureCase,productCompileAttempted=false,productRuntimeAttempted=false,fullTenMinuteElapsedValidation=false,networkAttempted=false});
        return passed?0:1;
    }
}
