using System;
using System.Diagnostics;
using System.IO;
using System.Threading;
class Fixture {
    static void WaitMarker(string path) {
        var timer=Stopwatch.StartNew();
        while(!File.Exists(path) && timer.ElapsedMilliseconds<5000) Thread.Sleep(10);
        if(!File.Exists(path)) throw new Exception("fixture readiness timeout");
    }
    static bool CapturedBoth(string directory) {
        var output=new FileInfo(Path.Combine(directory,"stdout.log"));
        var error=new FileInfo(Path.Combine(directory,"stderr.log"));
        return output.Exists && error.Exists && output.Length>0 && error.Length>0;
    }
    static int Main(string[] args) {
        string[] compileArgs={"test","--frozen","--offline","--bin","xenterm","--target","x86_64-pc-windows-msvc","--jobs","2","--no-run","--message-format=json"};
        if(System.Linq.Enumerable.SequenceEqual(args,compileArgs)) {
            string fixtureDirectory=Directory.GetCurrentDirectory();
            string fixtureMode=File.ReadAllText(Path.Combine(fixtureDirectory,"fixture-mode.txt"));
            if(fixtureMode!="normal" && fixtureMode!="overflow" && fixtureMode!="timeout") return 64;
            args=new[]{"root",fixtureMode,fixtureDirectory};
        }
        if(args.Length!=3) return 64;
        string role=args[0], mode=args[1], directory=args[2];
        if(!Path.IsPathFullyQualified(directory) || !Directory.Exists(directory)) return 64;
        if(role!="root" && role!="child" && role!="grandchild") return 64;
        File.WriteAllText(Path.Combine(directory,role+".pending"),Environment.ProcessId.ToString());
        File.Move(Path.Combine(directory,role+".pending"),Path.Combine(directory,role+".started"));
        if(role=="grandchild") {
            if(mode=="normal") return 0;
            Thread.Sleep(Timeout.Infinite);return 0;
        }
        var info=new ProcessStartInfo(Environment.ProcessPath) { UseShellExecute=false };
        info.ArgumentList.Add(role=="root"?"child":"grandchild");info.ArgumentList.Add(mode);info.ArgumentList.Add(directory);
        using(var child=Process.Start(info)) {
            if(child==null) return 65;
            WaitMarker(Path.Combine(directory,"grandchild.started"));
            if(role=="root" && mode=="parent-exit") return 0;
            if(role=="root" && mode=="parent-nonzero") return 7;
            if(role=="root" && mode=="overflow") {
                var block=new byte[4096];Array.Fill<byte>(block,65);
                var ready=new CountdownEvent(2);var start=new ManualResetEventSlim(false);
                var errorThread=new Thread(()=> {
                    using(var error=Console.OpenStandardError()) {
                        error.Write(block,0,block.Length);error.Flush();
                        ready.Signal();start.Wait();while(true) error.Write(block,0,block.Length);
                    }
                });
                errorThread.IsBackground=true;errorThread.Start();
                using(var output=Console.OpenStandardOutput()) {
                    output.Write(block,0,block.Length);output.Flush();ready.Signal();ready.Wait();
                    var captureReady=Stopwatch.StartNew();
                    while(!CapturedBoth(directory) && captureReady.ElapsedMilliseconds<5000) Thread.Sleep(10);
                    if(!CapturedBoth(directory)) return 66;
                    start.Set();while(true) output.Write(block,0,block.Length);
                }
            }
            child.WaitForExit();return child.ExitCode;
        }
    }
}
