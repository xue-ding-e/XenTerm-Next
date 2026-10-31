using System;
using System.IO;
using System.Linq;

// Synthetic cargo only: no network, compiler, package extraction or child spawn.
class Fixture {
    static int Main(string[] args) {
        if(!args.SequenceEqual(new[]{"fetch","--locked","--target","x86_64-pc-windows-msvc"})) return 64;
        string source=Directory.GetCurrentDirectory();
        string mode=File.ReadAllText(Path.Combine(source,"fixture-mode"));
        if(mode!="normal" && mode!="nonzero") return 65;
        File.AppendAllText(Path.Combine(source,"fixture.started"),Environment.ProcessId.ToString()+"\n");
        Console.Out.Write("synthetic stdout\n");Console.Error.Write("synthetic stderr\n");
        return mode=="normal" ? 0 : 7;
    }
}
