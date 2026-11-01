/// A tiny one-way bus that lets other screens (Snippets, command links) send
/// text to a running terminal. The target host id is explicit, so a message is
/// never delivered to the wrong PTY when several session tabs are alive.
library;

import 'package:flutter/foundation.dart';

class TerminalCommand {
  final String hostId;
  final String text;
  final bool sendEnter;

  const TerminalCommand({
    required this.hostId,
    required this.text,
    this.sendEnter = true,
  });
}

class TerminalCommandBus extends ChangeNotifier {
  TerminalCommand? _last;
  int _sequence = 0;

  TerminalCommand? get last => _last;
  int get sequence => _sequence;

  void send(String hostId, String text, {bool sendEnter = true}) {
    if (text.isEmpty) return;
    _last = TerminalCommand(
      hostId: hostId,
      text: text,
      sendEnter: sendEnter,
    );
    _sequence++;
    notifyListeners();
  }
}
