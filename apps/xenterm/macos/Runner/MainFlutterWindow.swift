import Cocoa
import FlutterMacOS

class MainFlutterWindow: NSWindow {
  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)

    // window_manager marks windows with a hidden titlebar as non-opaque, which
    // forces the compositor to blend every frame during live resize, zoom and
    // minimize. The Flutter surface covers the whole window, so the app asks us
    // to restore an opaque window with its current page colour after the window
    // options were applied.
    let channel = FlutterMethodChannel(
      name: "xenterm/window",
      binaryMessenger: flutterViewController.engine.binaryMessenger
    )
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self = self else {
        result(nil)
        return
      }
      guard call.method == "setOpaque" else {
        result(FlutterMethodNotImplemented)
        return
      }
      let args = call.arguments as? [String: Any] ?? [:]
      let red = (args["r"] as? NSNumber)?.doubleValue ?? 0
      let green = (args["g"] as? NSNumber)?.doubleValue ?? 0
      let blue = (args["b"] as? NSNumber)?.doubleValue ?? 0
      self.isOpaque = true
      self.backgroundColor = NSColor(
        srgbRed: red / 255.0,
        green: green / 255.0,
        blue: blue / 255.0,
        alpha: 1.0
      )
      result(nil)
    }

    super.awakeFromNib()
  }
}
