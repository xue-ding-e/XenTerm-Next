using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json;

// Candidate only. Controller qualification and stage readiness are separate gates.
class FetchHost {
    static readonly string[] FetchArgs={"fetch","--locked","--target","x86_64-pc-windows-msvc"};
    static readonly Dictionary<string,string> Homes=new Dictionary<string,string> {
        {"HOME","home"},{"USERPROFILE","home"},{"APPDATA","appdata"},
        {"LOCALAPPDATA","localappdata"},{"TEMP","tmp"},{"TMP","tmp"},
        {"CARGO_HOME","cargo-home"},{"RUSTUP_HOME","rustup-home"}};
    public sealed class Request {
        public string executable {get;set;} public string[] args {get;set;}
        public string cwd {get;set;} public int seconds {get;set;}
        public string stdout {get;set;} public string stderr {get;set;}
        public string commandPolicy {get;set;} public string completionMode {get;set;}
        public Dictionary<string,string> environment {get;set;}
    }
    static void Require(bool ok) {if(!ok) throw new ArgumentException("fetch gate");}
    static void Plain(string path,bool exists) {
        Require(!String.IsNullOrEmpty(path) && Path.IsPathFullyQualified(path) && Path.GetFullPath(path)==path);
        for(string p=path;p!=null;p=Path.GetDirectoryName(p)) {
            try {Require((File.GetAttributes(p)&FileAttributes.ReparsePoint)==0);}
            catch(FileNotFoundException) {Require(p==path && !exists);}
            catch(DirectoryNotFoundException) {Require(p==path && !exists);}
        }
    }
    static string ToolBin(string root) {return Path.Combine(root,"rustup-home","toolchains","1.99.1-x86_64-pc-windows-msvc","bin");}
    static void Validate(Request r,string root) {
        Require(r!=null && r.args!=null && r.environment!=null);
        Require(r.commandPolicy=="cargo-fetch-only-v1" && r.completionMode=="natural-tree" && r.seconds==600 && r.args.SequenceEqual(FetchArgs));
        Require(r.executable==Path.Combine(ToolBin(root),"cargo.exe") && r.cwd==Path.Combine(root,"source"));
        Require(r.stdout==Path.Combine(root,"fetch.stdout.log") && r.stderr==Path.Combine(root,"fetch.stderr.log"));
        Plain(r.executable,true);Plain(r.cwd,true);Plain(r.stdout,false);Plain(r.stderr,false);
        var expected=new Dictionary<string,string>(StringComparer.OrdinalIgnoreCase);
        foreach(var pair in Homes) {string p=Path.Combine(root,pair.Value);Plain(p,true);Require(Directory.Exists(p));expected.Add(pair.Key,p);}
        string os=Environment.GetFolderPath(Environment.SpecialFolder.Windows);Plain(os,true);
        expected.Add("SYSTEMROOT",os);expected.Add("WINDIR",os);
        expected.Add("PATH",ToolBin(root)+";"+Path.Combine(os,"System32"));
        expected.Add("RUSTC",Path.Combine(ToolBin(root),"rustc.exe"));Plain(expected["RUSTC"],true);
        expected.Add("CARGO_HTTP_TIMEOUT","30");expected.Add("CARGO_NET_RETRY","0");expected.Add("CARGO_REGISTRIES_CRATES_IO_PROTOCOL","sparse");
        Require(r.environment.Count==expected.Count);
        var keys=new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach(var pair in r.environment) Require(keys.Add(pair.Key) && expected.TryGetValue(pair.Key,out string v) && pair.Value==v);
        Require(File.Exists(r.executable) && File.Exists(expected["RUSTC"]));
    }
    static int Main(string[] args) {
        // No controller invokes this candidate until separate native qualification
        // and acquisition config/network/disk boundaries have been established.
        if(!OperatingSystem.IsWindows() || args.Length!=1) return 64;
        try {
            string root=args[0];Plain(root,true);
            string input=Path.Combine(root,"fetch.request.json");Plain(input,true);
            Require(new FileInfo(input).Length<=131072);
            string text=File.ReadAllText(input);using var json=JsonDocument.Parse(text);
            var required=new HashSet<string>(new[]{"executable","args","cwd","seconds","stdout","stderr","commandPolicy","completionMode","environment"});
            var names=json.RootElement.EnumerateObject().Select(p=>p.Name).ToArray();Require(names.Length==required.Count && required.SetEquals(names));
            var envKeys=new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach(var item in json.RootElement.GetProperty("environment").EnumerateObject()) Require(envKeys.Add(item.Name));
            var r=JsonSerializer.Deserialize<Request>(text);Validate(r,root);
            string receipt=Path.Combine(root,"fetch.receipt.json");Plain(receipt,false);Require(!File.Exists(receipt));
            // Persistent CreateNew ownership; even failure/unknown cleanup consumes root.
            string marker=Path.Combine(root,"fetch-started.marker");Plain(marker,false);
            using(var f=new FileStream(marker,FileMode.CreateNew,FileAccess.Write,FileShare.None)) {f.WriteByte(1);}
            var v=FetchOwnedProcess.Run(r.executable,r.args,r.cwd,600,r.stdout,r.stderr,r.environment,"natural-tree","cargo-fetch-only-v1");
            Plain(r.stdout,true);Plain(r.stderr,true);
            long outputBytes=new FileInfo(r.stdout).Length+new FileInfo(r.stderr).Length;
            Require(outputBytes<=1024*1024 && outputBytes<=v.outputBudget);
            // Raw Host lifecycle only. A separately qualified controller must add
            // its same-run/input/cache proof; never accept those from this request.
            using(var f=new FileStream(receipt,FileMode.CreateNew,FileAccess.Write,FileShare.None))
                JsonSerializer.Serialize(f,new {schema=1,state="returned",seconds=600,outputBytes,succeeded=v.Succeeded,v.exitCode,v.timedOut,v.outputExceeded,v.captureFailed,v.treeEmpty,v.forcedOwnedCleanup,v.completionMode,v.commandPolicy,v.outputBudget,v.cleanupMilliseconds,v.boundaryRootSignaled,v.boundaryActiveProcessCount,v.boundaryRootExitCode,v.boundaryRootExitCodeKnown,safeToStart=FetchOwnedProcess.SafeToStart,reusable=false,productCompileAttempted=false,productRuntimeAttempted=false});
            return v.Succeeded && FetchOwnedProcess.SafeToStart && !v.forcedOwnedCleanup ? 0 : 1;
        } catch {return 1;}
    }
}
