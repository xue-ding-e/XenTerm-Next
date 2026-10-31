using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json;
class CompileOnlyHost {
    const string MainSha="459b593c830b8be643df29f9d9c0c46915d54696";
    static readonly string[] CompileArgs={"test","--frozen","--offline","--bin","xenterm","--target","x86_64-pc-windows-msvc","--jobs","2","--no-run","--message-format=json"};
    public sealed class Request {
        public string executable {get;set;} public string[] args {get;set;}
        public string cwd {get;set;} public int seconds {get;set;}
        public string stdout {get;set;} public string stderr {get;set;}
        public string label {get;set;} public string commandPolicy {get;set;}
        public string completionMode {get;set;} public Dictionary<string,string> environment {get;set;}
    }
    static void Require(bool value) {if(!value) throw new ArgumentException("compile-only gate");}
    static void PlainPath(string path,bool exists) {
        Require(!String.IsNullOrEmpty(path) && Path.IsPathFullyQualified(path));
        string full=Path.GetFullPath(path);Require(full==path);
        for(string p=full;p!=null;p=Path.GetDirectoryName(p)) {
            bool present=File.Exists(p)||Directory.Exists(p);
            if(p==full && exists) Require(present);
            if(present) Require((File.GetAttributes(p)&FileAttributes.ReparsePoint)==0);
        }
    }
    static void Validate(Request r,string root) {
        Require(r!=null && r.environment!=null && r.args!=null);
        Require(r.label=="key-import-compile-only" && r.commandPolicy=="cargo-compile-only-v2" && r.completionMode=="natural-tree" && r.seconds==2700 && r.args.SequenceEqual(CompileArgs));
        Require(r.cwd==Path.Combine(root,"source") && r.executable==Path.Combine(root,"rustup-home","toolchains","1.99.1-x86_64-pc-windows-msvc","bin","cargo.exe"));
        Require(r.stdout==Path.Combine(root,"compile.stdout.log") && r.stderr==Path.Combine(root,"compile.stderr.log"));
        PlainPath(r.cwd,true);PlainPath(r.executable,true);PlainPath(r.stdout,false);PlainPath(r.stderr,false);
        var env=new Dictionary<string,string>(StringComparer.OrdinalIgnoreCase);
        foreach(var pair in r.environment) {Require(!env.ContainsKey(pair.Key));env.Add(pair.Key,pair.Value);}
        var directories=new Dictionary<string,string> {{"HOME","home"},{"USERPROFILE","home"},{"APPDATA","appdata"},{"LOCALAPPDATA","localappdata"},{"XDG_CONFIG_HOME","config"},{"XDG_DATA_HOME","data"},{"XDG_CACHE_HOME","cache"},{"XDG_RUNTIME_DIR","runtime"},{"XENTERM_DATA_DIR","profile"},{"TEMP","tmp"},{"TMP","tmp"},{"TMPDIR","tmp"},{"CARGO_HOME","cargo-home"},{"RUSTUP_HOME","rustup-home"},{"CARGO_TARGET_DIR","target"}};
        foreach(var pair in directories) {Require(env.TryGetValue(pair.Key,out string value) && value==Path.Combine(root,pair.Value));PlainPath(value,true);}
        var constants=new Dictionary<string,string> {{"CARGO_NET_OFFLINE","true"},{"CARGO_BUILD_JOBS","2"},{"CARGO_INCREMENTAL","0"},{"CARGO_PROFILE_DEV_DEBUG","0"},{"CARGO_PROFILE_TEST_DEBUG","0"},{"GITHUB_SHA",MainSha},{"GITHUB_REF_TYPE","branch"},{"GITHUB_REF_NAME","validation/key-import-compile-only-20261030"},{"XT_VERSION_JSON",Path.Combine(root,"version.json")},{"RUSTC",Path.Combine(root,"rustup-home","toolchains","1.99.1-x86_64-pc-windows-msvc","bin","rustc.exe")}};
        foreach(var pair in constants) Require(env.TryGetValue(pair.Key,out string value) && value==pair.Value);
        PlainPath(env["RUSTC"],true);
        PlainPath(Path.Combine(root,"version.json"),false);
    }
    // Three fixed read-only setup queries; never invokes the compile-only path.
    static int QueryRust(string root) {
        PlainPath(root,true);
        string report=Path.Combine(root,"rust-query-receipts.json");PlainPath(report,false);Require(!File.Exists(report));
        var env=new Dictionary<string,string>(StringComparer.OrdinalIgnoreCase);
        foreach(var pair in new Dictionary<string,string>{{"HOME","home"},{"USERPROFILE","home"},{"APPDATA","appdata"},{"LOCALAPPDATA","localappdata"},{"TEMP","tmp"},{"TMP","tmp"},{"CARGO_HOME","cargo-home"},{"RUSTUP_HOME","rustup-home"}}) {
            string path=Path.Combine(root,pair.Value);PlainPath(path,true);env.Add(pair.Key,path);
        }
        string os=Environment.GetFolderPath(Environment.SpecialFolder.Windows);PlainPath(os,true);
        env.Add("SYSTEMROOT",os);env.Add("WINDIR",os);
        // Query real fixed toolchain files, never rustup's cargo/rustc reparse proxies.
        string toolchainBin=Path.Combine(root,"rustup-home","toolchains","1.99.1-x86_64-pc-windows-msvc","bin");PlainPath(toolchainBin,true);
        env.Add("PATH",toolchainBin+";"+Path.Combine(os,"System32"));
        string[] labels={"rustc-version","cargo-version","resolved-rustc"};
        string[] tools={Path.Combine(toolchainBin,"rustc.exe"),Path.Combine(toolchainBin,"cargo.exe"),Path.Combine(root,"cargo-home","bin","rustup.exe")};
        string[][] argv={new[]{"-vV"},new[]{"-V"},new[]{"which","--toolchain","1.99.1","rustc"}};
        var rows=new List<object>();bool all=false;
        try {
            for(int i=0;i<labels.Length;i++) {
                string exe=tools[i];PlainPath(exe,true);
                string stdout=Path.Combine(root,labels[i]+".stdout.log"),stderr=Path.Combine(root,labels[i]+".stderr.log");
                PlainPath(stdout,false);PlainPath(stderr,false);Require(!File.Exists(stdout)&&!File.Exists(stderr));
                var v=XenOwnedProcess.Run(exe,argv[i],root,20,stdout,stderr,env,"natural-tree","smoke-v1");
                rows.Add(new {label=labels[i],v.exitCode,v.timedOut,v.outputExceeded,v.captureFailed,v.treeEmpty,v.forcedOwnedCleanup,v.completionMode,v.commandPolicy,v.outputBudget,v.cleanupMilliseconds,safeToStart=XenOwnedProcess.SafeToStart,succeeded=v.Succeeded});
                Require(v.Succeeded && XenOwnedProcess.SafeToStart && new FileInfo(stdout).Length<=16384);
            }
            all=true;return 0;
        } finally {
            using(var file=new FileStream(report,FileMode.CreateNew,FileAccess.Write,FileShare.None)) JsonSerializer.Serialize(file,new {schema=1,completed=all,query_count=rows.Count,rows,productCompileAttempted=false,productRuntimeAttempted=false});
        }
    }
    static int Main(string[] args) {
        if(args.Length!=2 || !OperatingSystem.IsWindows()) return 64;
        if(args[0]=="--verify-rust-setup") {try{return QueryRust(args[1]);}catch{return 1;}}
        try {
            PlainPath(args[0],true);PlainPath(args[1],false);
            string root=Path.GetDirectoryName(args[0]);
            Require(args[0]==Path.Combine(root,"compile.request.json") && args[1]==Path.Combine(root,"compile.receipt.json") && !File.Exists(args[1]));
        } catch {return 64;}
        object receipt;int code=1;
        try {
            PlainPath(args[0],true);PlainPath(args[1],false);Require(!File.Exists(args[1]));
            string root=Path.GetDirectoryName(args[0]);
            Require(args[0]==Path.Combine(root,"compile.request.json") && args[1]==Path.Combine(root,"compile.receipt.json"));
            Require(new FileInfo(args[0]).Length<=131072);
            string text=File.ReadAllText(args[0]);using var json=JsonDocument.Parse(text);
            var expected=new HashSet<string>(new[]{"label","commandPolicy","completionMode","executable","args","cwd","seconds","stdout","stderr","environment"});
            var keys=json.RootElement.EnumerateObject().Select(p=>p.Name).ToArray();Require(keys.Length==expected.Count && expected.SetEquals(keys));
            var environment=json.RootElement.GetProperty("environment");Require(environment.ValueKind==JsonValueKind.Object);
            var environmentKeys=new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach(var pair in environment.EnumerateObject()) Require(environmentKeys.Add(pair.Name));
            var r=JsonSerializer.Deserialize<Request>(text);Validate(r,root);
            var v=XenOwnedProcess.Run(r.executable,r.args,r.cwd,r.seconds,r.stdout,r.stderr,r.environment,r.completionMode,r.commandPolicy);
            long outputBytes=new FileInfo(r.stdout).Length+new FileInfo(r.stderr).Length;
            receipt=new {schema=1,state="returned",v.commandPolicy,v.completionMode,v.outputBudget,outputBytes,v.cleanupMilliseconds,v.exitCode,v.timedOut,v.outputExceeded,v.captureFailed,v.treeEmpty,v.forcedOwnedCleanup,v.boundaryRootSignaled,v.boundaryActiveProcessCount,v.boundaryRootExitCodeKnown,v.boundaryRootExitCode,safeToStart=XenOwnedProcess.SafeToStart,succeeded=v.Succeeded,productCompileAttempted=true,productRuntimeAttempted=false};code=0;
        } catch(Exception e) {
            receipt=new {schema=1,state="host-error",errorType=e.GetType().Name,treeEmpty=false,safeToStart=XenOwnedProcess.SafeToStart,productCompileAttempted="unknown",productRuntimeAttempted=false};
        }
        // Unknown outer Host termination yields no receipt; caller must fail/poison.
        using(var file=new FileStream(args[1],FileMode.CreateNew,FileAccess.Write,FileShare.None)) JsonSerializer.Serialize(file,receipt);
        return code;
    }
}
