using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text.Json;
using System.Threading.Tasks;

// Invokes the unmodified production Host with a synthetic cargo at its exact path.
// This is NOT a real-fetch launcher or a replacement production timeout policy.
class HostEntryHarness {
    static string host,fixture,root,active="initial";
    static int hostStarts=0;
    static bool outerTimeoutObserved=false;
    static readonly List<object> cases=new List<object>();
    static readonly string[] RuntimeSuffixes={".exe",".dll",".deps.json",".runtimeconfig.json"};
    static void Need(bool value) {if(!value) throw new InvalidOperationException("synthetic Host contract");}
    static string Hash(string path) {return Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(path)));}
    static string Bin(string stage) {return Path.Combine(stage,"rustup-home","toolchains","1.99.1-x86_64-pc-windows-msvc","bin");}
    static string Stage(string name,string mode="normal") {
        active=name;string stage=Path.Combine(root,name);Need(!Directory.Exists(stage));Directory.CreateDirectory(stage);
        foreach(string part in new[]{"source","home","appdata","localappdata","tmp","cargo-home","rustup-home"}) Directory.CreateDirectory(Path.Combine(stage,part));
        Directory.CreateDirectory(Bin(stage));
        foreach(string suffix in RuntimeSuffixes) File.Copy(Path.Combine(Path.GetDirectoryName(fixture),"cargo"+suffix),Path.Combine(Bin(stage),"cargo"+suffix));
        // Host checks the fixed rustc file; synthetic cargo never invokes it.
        File.WriteAllBytes(Path.Combine(Bin(stage),"rustc.exe"),new byte[]{0});
        File.WriteAllText(Path.Combine(stage,"source","fixture-mode"),mode);
        return stage;
    }
    static Dictionary<string,string> EnvironmentFor(string stage) {
        string windows=Environment.GetFolderPath(Environment.SpecialFolder.Windows);
        return new Dictionary<string,string> {
            {"HOME",Path.Combine(stage,"home")},{"USERPROFILE",Path.Combine(stage,"home")},
            {"APPDATA",Path.Combine(stage,"appdata")},{"LOCALAPPDATA",Path.Combine(stage,"localappdata")},
            {"TEMP",Path.Combine(stage,"tmp")},{"TMP",Path.Combine(stage,"tmp")},
            {"CARGO_HOME",Path.Combine(stage,"cargo-home")},{"RUSTUP_HOME",Path.Combine(stage,"rustup-home")},
            {"SYSTEMROOT",windows},{"WINDIR",windows},{"PATH",Bin(stage)+";"+Path.Combine(windows,"System32")},
            {"RUSTC",Path.Combine(Bin(stage),"rustc.exe")},{"CARGO_HTTP_TIMEOUT","30"},
            {"CARGO_NET_RETRY","0"},{"CARGO_REGISTRIES_CRATES_IO_PROTOCOL","sparse"}};
    }
    static Dictionary<string,object> Request(string stage) {
        return new Dictionary<string,object> {
            {"executable",Path.Combine(Bin(stage),"cargo.exe")},{"args",new[]{"fetch","--locked","--target","x86_64-pc-windows-msvc"}},
            {"cwd",Path.Combine(stage,"source")},{"seconds",600},{"stdout",Path.Combine(stage,"fetch.stdout.log")},
            {"stderr",Path.Combine(stage,"fetch.stderr.log")},{"commandPolicy","cargo-fetch-only-v1"},
            {"completionMode","natural-tree"},{"environment",EnvironmentFor(stage)}};
    }
    static void WriteRequest(string stage,Dictionary<string,object> request) {
        File.WriteAllText(Path.Combine(stage,"fetch.request.json"),JsonSerializer.Serialize(request));
    }
    static int RunHost(string stage) {
        var info=new ProcessStartInfo(host) {UseShellExecute=false,WorkingDirectory=stage,RedirectStandardOutput=true,RedirectStandardError=true};
        info.Environment.Clear();
        string windows=Environment.GetFolderPath(Environment.SpecialFolder.Windows);
        info.Environment.Add("SYSTEMROOT",windows);info.Environment.Add("WINDIR",windows);
        info.ArgumentList.Add(stage);
        using(var process=Process.Start(info)) {
            Need(process!=null);
            hostStarts++;
            Task[] drain={process.StandardOutput.BaseStream.CopyToAsync(Stream.Null),process.StandardError.BaseStream.CopyToAsync(Stream.Null)};
            if(!process.WaitForExit(20000)) {
                // Synthetic outer timeout is a failed/unknown lifecycle, never a pass.
                outerTimeoutObserved=true;
                try {process.Kill(true);process.WaitForExit(5000);} finally {throw new InvalidOperationException("synthetic Host outer timeout");}
            }
            Need(Task.WaitAll(drain,5000));
            return process.ExitCode;
        }
    }
    static int Starts(string stage) {
        string marker=Path.Combine(stage,"source","fixture.started");
        return File.Exists(marker) ? File.ReadAllLines(marker).Length : 0;
    }
    static void Receipt(string stage,int expectedExit) {
        using(var doc=JsonDocument.Parse(File.ReadAllText(Path.Combine(stage,"fetch.receipt.json")))) {
            var r=doc.RootElement;
            Need(r.GetProperty("state").GetString()=="returned" && r.GetProperty("seconds").GetInt32()==600);
            Need(r.GetProperty("commandPolicy").GetString()=="cargo-fetch-only-v1" && r.GetProperty("completionMode").GetString()=="natural-tree");
            Need(r.GetProperty("exitCode").GetInt32()==expectedExit && r.GetProperty("boundaryRootExitCode").GetInt32()==expectedExit);
            Need(r.GetProperty("boundaryActiveProcessCount").GetInt32()==0);
            foreach(string key in new[]{"treeEmpty","safeToStart","boundaryRootSignaled","boundaryRootExitCodeKnown"}) Need(r.GetProperty(key).GetBoolean());
            foreach(string key in new[]{"timedOut","outputExceeded","captureFailed","forcedOwnedCleanup","reusable","productCompileAttempted","productRuntimeAttempted"}) Need(!r.GetProperty(key).GetBoolean());
            Need(r.GetProperty("succeeded").GetBoolean()==(expectedExit==0));
            Need(r.GetProperty("cleanupMilliseconds").GetInt64()>=0 && r.GetProperty("cleanupMilliseconds").GetInt64()<=5000);
            long bytes=new FileInfo(Path.Combine(stage,"fetch.stdout.log")).Length+new FileInfo(Path.Combine(stage,"fetch.stderr.log")).Length;
            Need(bytes>0 && r.GetProperty("outputBytes").GetInt64()==bytes && r.GetProperty("outputBudget").GetInt64()==1024*1024 && bytes<=1024*1024);
        }
        Need(File.ReadAllBytes(Path.Combine(stage,"fetch-started.marker")).SequenceEqual(new byte[]{1}));
    }
    static void Rejection(string name,Action<string,Dictionary<string,object>> mutate) {
        string stage=Stage(name);var request=Request(stage);mutate(stage,request);WriteRequest(stage,request);
        Need(RunHost(stage)==1 && Starts(stage)==0);
        Need(!File.Exists(Path.Combine(stage,"fetch-started.marker")) && !File.Exists(Path.Combine(stage,"fetch.receipt.json")));
        cases.Add(new{name,passed=true,syntheticCargoExecutions=0,expectedRejection=true});
    }
    static void DuplicateJson() {
        string stage=Stage("duplicate-json");string text=JsonSerializer.Serialize(Request(stage));
        File.WriteAllText(Path.Combine(stage,"fetch.request.json"),"{\"seconds\":600,"+text.Substring(1));
        Need(RunHost(stage)==1 && Starts(stage)==0 && !File.Exists(Path.Combine(stage,"fetch-started.marker")));
        Need(!File.Exists(Path.Combine(stage,"fetch.receipt.json")));
        cases.Add(new{name=active,passed=true,syntheticCargoExecutions=0,expectedRejection=true});
    }
    static void Reparse() {
        string stage=Stage("reparse-executable");string cargo=Path.Combine(Bin(stage),"cargo.exe");File.Delete(cargo);
        // Link creation failure is a failed suite, never a skipped successful case.
        File.CreateSymbolicLink(cargo,fixture);
        WriteRequest(stage,Request(stage));Need(RunHost(stage)==1 && Starts(stage)==0);
        Need(!File.Exists(Path.Combine(stage,"fetch-started.marker")) && !File.Exists(Path.Combine(stage,"fetch.receipt.json")));
        cases.Add(new{name=active,passed=true,syntheticCargoExecutions=0,expectedRejection=true});
    }
    static int Main(string[] args) {
        if(!OperatingSystem.IsWindows() || args.Length!=3) return 64;
        host=Path.GetFullPath(args[0]);fixture=Path.GetFullPath(args[1]);root=Path.GetFullPath(args[2]);
        if(Path.GetFileName(host)!="FetchHost.exe" || Path.GetFileName(fixture)!="cargo.exe" || !File.Exists(host) || !File.Exists(fixture) || Directory.Exists(root)) return 64;
        Directory.CreateDirectory(root);bool passed=false;string failureCase=null;
        var before=new Dictionary<string,string>();
        foreach(string exe in new[]{host,fixture}) foreach(string suffix in RuntimeSuffixes) {
            string path=Path.Combine(Path.GetDirectoryName(exe),Path.GetFileNameWithoutExtension(exe)+suffix);before.Add(path,Hash(path));
        }
        try {
            string stage=Stage("fixed-success");WriteRequest(stage,Request(stage));Need(RunHost(stage)==0 && Starts(stage)==1);Receipt(stage,0);
            cases.Add(new{name=active,passed=true,syntheticCargoExecutions=1,expectedRejection=false});
            active="consumed-root";string receiptHash=Hash(Path.Combine(stage,"fetch.receipt.json"));
            Need(RunHost(stage)==1 && Starts(stage)==1 && Hash(Path.Combine(stage,"fetch.receipt.json"))==receiptHash);
            cases.Add(new{name=active,passed=true,syntheticCargoExecutions=0,expectedRejection=true});
            stage=Stage("marker-only-consumed-root");WriteRequest(stage,Request(stage));
            File.WriteAllBytes(Path.Combine(stage,"fetch-started.marker"),new byte[]{1});
            Need(RunHost(stage)==1 && Starts(stage)==0 && !File.Exists(Path.Combine(stage,"fetch.receipt.json")));
            Need(!File.Exists(Path.Combine(stage,"fetch.stdout.log")) && !File.Exists(Path.Combine(stage,"fetch.stderr.log")));
            Need(File.ReadAllBytes(Path.Combine(stage,"fetch-started.marker")).SequenceEqual(new byte[]{1}));
            cases.Add(new{name=active,passed=true,syntheticCargoExecutions=0,expectedRejection=true});
            Rejection("compile-argv",(s,r)=>r["args"]=new[]{"test","--no-run"});
            Rejection("wrong-policy",(s,r)=>r["commandPolicy"]="smoke-v1");
            Rejection("wrong-deadline",(s,r)=>r["seconds"]=601);
            Rejection("extra-environment",(s,r)=>((Dictionary<string,string>)r["environment"]).Add("CARGO_REGISTRY_TOKEN","synthetic-not-secret"));
            Rejection("case-colliding-environment",(s,r)=>((Dictionary<string,string>)r["environment"]).Add("path","duplicate"));
            Rejection("wrong-tool-path",(s,r)=>r["executable"]=fixture);
            DuplicateJson();Reparse();
            stage=Stage("nonzero-child","nonzero");WriteRequest(stage,Request(stage));Need(RunHost(stage)==1 && Starts(stage)==1);Receipt(stage,7);
            cases.Add(new{name=active,passed=true,syntheticCargoExecutions=1,expectedRejection=false});
            active="nonzero-consumed-root";Need(RunHost(stage)==1 && Starts(stage)==1);
            cases.Add(new{name=active,passed=true,syntheticCargoExecutions=0,expectedRejection=true});
            Need(cases.Count==13);passed=true;
        } catch {failureCase=active;} finally {
            foreach(var item in before) {if(!File.Exists(item.Key) || Hash(item.Key)!=item.Value) {passed=false;failureCase="runtime-identity";}}
            using(var output=new FileStream(Path.Combine(root,"host-entry-result.json"),FileMode.CreateNew,FileAccess.Write))
                JsonSerializer.Serialize(output,new {passed,case_count=cases.Count,cases,failureCase,hostEntryExecuted=hostStarts>0,hostOsProcessInvocations=hostStarts,outerTimeoutObserved,outerJobObjectQualification=false,realCargoExecuted=false,dependencyFetchAttempted=false,fixtureNetworkAttempted=false,productCompileAttempted=false,productRuntimeAttempted=false});
        }
        return passed ? 0 : 1;
    }
}
