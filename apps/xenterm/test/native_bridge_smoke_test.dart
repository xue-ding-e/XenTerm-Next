// Release-bundle smoke test: proves the packaged bridge can be opened by the
// platform dynamic loader and that the C ABI version export works. CI sets
// XENTERM_BRIDGE_PATH to the staged library (macOS Frameworks dylib, Linux
// bundle/lib .so, Windows dll); other environments skip the test.
library;

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';

void main() {
  final path = Platform.environment['XENTERM_BRIDGE_PATH'];

  test(
    'the packaged bridge loads and returns its version',
    () {
      final bridge = NativeBridge.tryLoad();
      expect(bridge, isNotNull, reason: NativeBridge.loadError);
      final expected = Platform.environment['XT_VERSION'];
      expect(expected, isNotNull, reason: 'Packaged smoke test requires XT_VERSION');
      expect(expected, isNotEmpty);
      expect(bridge!.version(), expected);
    },
    skip: path != null && path.isNotEmpty
        ? false
        : 'only runs when CI sets XENTERM_BRIDGE_PATH to the packaged bridge',
  );
}
