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
    static int Main(string[] args) {
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
            if(role=="root" && mode=="overflow") {
                var block=new byte[4096];Array.Fill<byte>(block,65);
                using(var output=Console.OpenStandardOutput()) while(true) output.Write(block,0,block.Length);
            }
            child.WaitForExit();return child.ExitCode;
        }
    }
}
