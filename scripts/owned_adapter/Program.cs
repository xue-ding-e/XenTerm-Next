using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json;
class OwnedAdapter {
    public sealed class Request {
        public string executable {get;set;}
        public string label {get;set;}
        public string completionMode {get;set;}
        public string[] args {get;set;}
        public string cwd {get;set;}
        public int seconds {get;set;}
        public string stdout {get;set;}
        public string stderr {get;set;}
        public Dictionary<string,string> environment {get;set;}
    }
    static void ValidateCompletion(Request r) {
        if(r.label!="msvc-link-proof") {
            if(r.completionMode!="natural-tree") throw new ArgumentException("non-smoke mode");
            return;
        }
        var a=r.args;string output=Path.GetDirectoryName(r.stdout);
        string source=Path.Combine(output,"msvc-link-proof","probe.rs"),binary=Path.Combine(output,"msvc-link-proof","probe.exe");
        if(r.completionMode!="root-completed-then-owned-cleanup" || r.seconds!=20 || a==null || a.Length!=12 || !String.Equals(Path.GetFileName(r.executable),"rustc.exe",StringComparison.OrdinalIgnoreCase)) throw new ArgumentException("empty compiler mode");
        string[] prefix={"+1.99.1","--target","x86_64-pc-windows-msvc","--crate-name","synthetic_msvc_proof","--crate-type","bin","-C"};
        if(!a.Take(8).SequenceEqual(prefix) || a[8]==null || !a[8].StartsWith("linker=",StringComparison.Ordinal) || !Path.IsPathFullyQualified(a[8].Substring(7)) || !String.Equals(Path.GetFileName(a[8].Substring(7)),"link.exe",StringComparison.OrdinalIgnoreCase)) throw new ArgumentException("empty compiler arguments");
        if(a[9]!=source || a[10]!="-o" || a[11]!=binary || File.Exists(binary) || !File.Exists(source) || !File.ReadAllBytes(source).SequenceEqual(System.Text.Encoding.ASCII.GetBytes("fn main() {}\n"))) throw new ArgumentException("empty source identity");
    }
    static int Main(string[] args) {
        if(args.Length!=2 || !OperatingSystem.IsWindows()) return 64;
        if(!Path.IsPathFullyQualified(args[0]) || !Path.IsPathFullyQualified(args[1]) || File.Exists(args[1])) return 64;
        object receipt;int code=1;
        try {
            var file=new FileInfo(args[0]);
            if(!file.Exists || file.Length>131072) throw new ArgumentException("request bound");
            string text=File.ReadAllText(args[0]);using var document=JsonDocument.Parse(text);
            var expected=new HashSet<string>(new[]{"label","completionMode","executable","args","cwd","seconds","stdout","stderr","environment"});
            var keys=document.RootElement.EnumerateObject().Select(p=>p.Name).ToArray();
            if(keys.Length!=expected.Count || !expected.SetEquals(keys)) throw new ArgumentException("request schema");
            var request=JsonSerializer.Deserialize<Request>(text);
            if(request==null) throw new ArgumentException("request absent");
            ValidateCompletion(request);
            var result=XenOwnedProcess.Run(request.executable,request.args,request.cwd,request.seconds,request.stdout,request.stderr,request.environment,request.completionMode);
            receipt=new {schema=1,state="returned",result.completionMode,result.forcedOwnedCleanup,result.exitCode,result.boundaryRootSignaled,result.boundaryActiveProcessCount,result.boundaryRootExitCodeKnown,result.boundaryRootExitCode,result.timedOut,result.outputExceeded,result.treeEmpty,result.captureFailed,succeeded=result.Succeeded,safeToStart=XenOwnedProcess.SafeToStart,productExecuted=false};
            code=0; // Python separately checks all child flags and the requested command's exit.
        } catch(Exception error) {
            receipt=new {schema=1,state="host-error",errorType=error.GetType().Name,treeEmpty=false,safeToStart=XenOwnedProcess.SafeToStart,productExecuted=false};
        }
        using(var output=new FileStream(args[1],FileMode.CreateNew,FileAccess.Write,FileShare.None)) JsonSerializer.Serialize(output,receipt);
        return code;
    }
}
