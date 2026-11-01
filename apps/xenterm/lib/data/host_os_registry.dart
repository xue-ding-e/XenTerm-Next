/// The operating system each host reported on its last connection.
///
/// Detected from the SSH monitor channel (`/etc/os-release` or `uname -s`) and
/// kept in memory so Home's cards show the real logo after a connect. It is
/// intentionally not persisted: every new connection probes again.
library;

import 'package:flutter/foundation.dart';

import 'hosts.dart';

/// Maps the core's OS token (`debian`, `ubuntu`, `macos`, `windows`, …) onto
/// the card's chip.
HostOs? hostOsFromCore(String? name) {
  final value = (name ?? '').trim().toLowerCase();
  if (value.isEmpty) return null;
  if (value.contains('ubuntu')) return HostOs.ubuntu;
  if (value.contains('debian') ||
      value.contains('mint') ||
      value.contains('kali')) {
    return HostOs.debian;
  }
  if (value.contains('arch')) return HostOs.arch;
  if (value.contains('darwin') ||
      value.contains('macos') ||
      value.contains('apple')) {
    return HostOs.apple;
  }
  if (value.contains('windows') ||
      value.contains('mingw') ||
      value.contains('cygwin') ||
      value.contains('msys')) {
    return HostOs.windows;
  }
  if (value.contains('android')) return HostOs.android;
  if (value.contains('linux')) return HostOs.linux;
  return null;
}

class HostOsRegistry extends ChangeNotifier {
  final Map<String, HostOs> _byId = {};

  HostOs? of(String id) => _byId[id];

  /// Records a detection; unknown names leave the current icon untouched.
  void record(String id, String? coreName) {
    final detected = hostOsFromCore(coreName);
    if (detected == null || _byId[id] == detected) return;
    _byId[id] = detected;
    notifyListeners();
  }
}

final hostOsRegistry = HostOsRegistry();
