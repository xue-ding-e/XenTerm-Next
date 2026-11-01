import 'dart:convert';
import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:path_provider/path_provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

class ImportedFont {
  final String name;
  final String family;
  final String path;

  const ImportedFont(this.name, this.family, this.path);

  Map<String, String> toJson() => {
    'name': name,
    'family': family,
    'path': path,
  };
}

/// Owns UI/terminal font choices. Imported font bytes stay in the application's
/// support directory; only labels/paths and selections live in preferences.
/// TerminalView reads these values for its cell measurement and painting.
class AppFonts extends ChangeNotifier {
  static const _key = 'xenterm.fonts.v1';
  final List<ImportedFont> imported = [];
  String uiFamily = '';
  String terminalFamily = '';
  double uiSize = 14;
  double terminalSize = 15;
  String? error;

  static double bound(double size) => size.clamp(10, 28).toDouble();

  String labelFor(String family) {
    if (family.isEmpty) return '默认';
    for (final font in imported) {
      if (font.family == family) return font.name;
    }
    return family;
  }

  void setUiFamily(String family) {
    uiFamily = family;
    notifyListeners();
    _save();
  }

  void setTerminalFamily(String family) {
    terminalFamily = family;
    notifyListeners();
    _save();
  }

  void setUiSize(double size) {
    uiSize = bound(size);
    notifyListeners();
    _save();
  }

  void setTerminalSize(double size) {
    terminalSize = bound(size);
    notifyListeners();
    _save();
  }

  Future<void> load() async {
    final prefs = await SharedPreferences.getInstance();
    final raw = prefs.getString(_key);
    if (raw == null) return;
    try {
      final saved = jsonDecode(raw) as Map<String, dynamic>;
      uiSize = bound((saved['uiSize'] as num?)?.toDouble() ?? 14);
      terminalSize = bound((saved['terminalSize'] as num?)?.toDouble() ?? 15);
      for (final entry in (saved['imported'] as List<dynamic>? ?? [])) {
        final item = entry as Map<String, dynamic>;
        final font = ImportedFont(
          item['name'] as String,
          item['family'] as String,
          item['path'] as String,
        );
        if (await File(font.path).exists()) {
          try {
            await _register(font.family, await File(font.path).readAsBytes());
            imported.add(font);
          } catch (_) {
            // A removed or corrupt font cannot be selected again.
          }
        }
      }
      final families = imported.map((font) => font.family).toSet();
      final ui = saved['uiFamily'];
      final terminal = saved['terminalFamily'];
      uiFamily = ui is String && families.contains(ui) ? ui : '';
      terminalFamily = terminal is String && families.contains(terminal)
          ? terminal
          : '';
    } catch (_) {
      error = '字体设置读取失败，已恢复默认字体';
    }
    notifyListeners();
  }

  Future<bool> importFont() async {
    error = null;
    try {
      const type = XTypeGroup(label: '字体', extensions: ['ttf', 'otf']);
      final selected = await openFile(acceptedTypeGroups: [type]);
      if (selected == null) return false;
      final bytes = await selected.readAsBytes();
      if (bytes.length < 12 ||
          bytes.length > 20 * 1024 * 1024 ||
          !_validHeader(bytes)) {
        throw const FormatException('仅支持 20 MB 以下的 TTF/OTF 字体');
      }
      final dir = Directory(
        '${(await getApplicationSupportDirectory()).path}/fonts',
      );
      await dir.create(recursive: true);
      final family = 'xenterm_${DateTime.now().microsecondsSinceEpoch}';
      final path =
          '${dir.path}/$family.${selected.name.toLowerCase().endsWith('.otf') ? 'otf' : 'ttf'}';
      final file = File(path);
      await file.writeAsBytes(bytes, flush: true);
      try {
        await _register(family, bytes);
      } catch (_) {
        await file.delete();
        rethrow;
      }
      imported.add(ImportedFont(selected.name, family, path));
      await _save();
      notifyListeners();
      return true;
    } catch (e) {
      error = '导入失败：$e';
      notifyListeners();
      return false;
    }
  }

  static bool _validHeader(Uint8List bytes) {
    final tag = bytes.sublist(0, 4);
    return (tag[0] == 0 && tag[1] == 1 && tag[2] == 0 && tag[3] == 0) ||
        ascii.decode(tag, allowInvalid: true) == 'OTTO';
  }

  static Future<void> _register(String family, Uint8List bytes) async {
    final loader = FontLoader(family);
    loader.addFont(Future.value(ByteData.sublistView(bytes)));
    await loader.load();
  }

  Future<void> _save() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(
      _key,
      jsonEncode({
        'uiFamily': uiFamily,
        'terminalFamily': terminalFamily,
        'uiSize': uiSize,
        'terminalSize': terminalSize,
        'imported': imported.map((font) => font.toJson()).toList(),
      }),
    );
  }
}
