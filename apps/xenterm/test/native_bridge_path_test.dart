import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';

void main() {
  final sep = Platform.pathSeparator;

  String tempPath(List<String> parts) =>
      [Directory.systemTemp.path, ...parts].join(sep);

  test('bridge library is resolved alongside the executable, not the cwd', () {
    final executable = tempPath(['XenTerm build', 'bundle', 'xenterm_app']);
    final expected = tempPath(['XenTerm build', 'bundle', 'libxenterm_bridge.so']);

    expect(
      NativeBridge.libraryPathForExecutable(executable, 'libxenterm_bridge.so'),
      expected,
    );
    expect(expected, isNot('libxenterm_bridge.so'));
  });

  test('macOS looks in Contents/Frameworks before Contents/MacOS', () {
    final executable = tempPath([
      'XenTerm.app',
      'Contents',
      'MacOS',
      'xenterm_app',
    ]);
    final paths = NativeBridge.librarySearchPaths(
      executable,
      'libxenterm_bridge.dylib',
      isMacOS: true,
      isLinux: false,
    );

    expect(paths, [
      tempPath([
        'XenTerm.app',
        'Contents',
        'Frameworks',
        'libxenterm_bridge.dylib',
      ]),
      tempPath(['XenTerm.app', 'Contents', 'MacOS', 'libxenterm_bridge.dylib']),
    ]);
  });

  test('Linux looks next to the executable and in the bundle lib directory', () {
    final executable = tempPath(['bundle', 'xenterm']);
    final paths = NativeBridge.librarySearchPaths(
      executable,
      'libxenterm_bridge.so',
      isMacOS: false,
      isLinux: true,
    );

    expect(paths, [
      tempPath(['bundle', 'libxenterm_bridge.so']),
      tempPath(['bundle', 'lib', 'libxenterm_bridge.so']),
    ]);
  });

  test('macOS release layout with spaces and non-ASCII keeps priority', () {
    // The packaged app is `XenTerm.app`; users may unzip it into folders like
    // "下载 目录/我的 应用". URI resolution must still produce usable paths and
    // the Frameworks copy must win over a stray file next to the executable.
    final executable = tempPath([
      '下载 目录',
      '我的 应用',
      'XenTerm.app',
      'Contents',
      'MacOS',
      'xenterm_app',
    ]);
    final paths = NativeBridge.librarySearchPaths(
      executable,
      'libxenterm_bridge.dylib',
      isMacOS: true,
      isLinux: false,
    );

    expect(paths, [
      tempPath([
        '下载 目录',
        '我的 应用',
        'XenTerm.app',
        'Contents',
        'Frameworks',
        'libxenterm_bridge.dylib',
      ]),
      tempPath([
        '下载 目录',
        '我的 应用',
        'XenTerm.app',
        'Contents',
        'MacOS',
        'libxenterm_bridge.dylib',
      ]),
    ]);
    for (final path in paths) {
      expect(File(path).uri.scheme, 'file');
      expect(path.contains('%20'), isFalse);
    }
  });

  test('Linux release layout resolves the bundled lib directory', () {
    final executable = tempPath([
      '我的 应用',
      'xenterm-0.7.3-linux-x86_64-desktop',
      'xenterm',
    ]);
    final paths = NativeBridge.librarySearchPaths(
      executable,
      'libxenterm_bridge.so',
      isMacOS: false,
      isLinux: true,
    );

    expect(paths, [
      tempPath([
        '我的 应用',
        'xenterm-0.7.3-linux-x86_64-desktop',
        'libxenterm_bridge.so',
      ]),
      tempPath([
        '我的 应用',
        'xenterm-0.7.3-linux-x86_64-desktop',
        'lib',
        'libxenterm_bridge.so',
      ]),
    ]);
  });

  test('Windows only looks next to the executable', () {
    final executable = tempPath(['release', 'xenterm.exe']);
    final paths = NativeBridge.librarySearchPaths(
      executable,
      'xenterm_bridge.dll',
      isMacOS: false,
      isLinux: false,
    );

    expect(paths, [tempPath(['release', 'xenterm_bridge.dll'])]);
  });
}

