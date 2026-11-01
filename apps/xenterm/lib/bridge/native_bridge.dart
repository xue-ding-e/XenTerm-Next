/// The Dart side of the FFI bridge onto the Rust service core
/// (docs/flutter-bridge.md, step 1).
///
/// Strings cross as UTF-8 C pointers owned by the caller; the native side
/// frees them through `xenterm_string_free`. `ConfigStore::load` touches the
/// OS keyring and SQLite, so loading runs on a worker isolate, never on the
/// UI thread.
library;

import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:ffi/ffi.dart';

import '../data/hosts.dart';

typedef _PtrFnC = Pointer<Utf8> Function();
typedef _PtrFn = Pointer<Utf8> Function();
typedef _FreeFnC = Void Function(Pointer<Utf8>);
typedef _FreeFn = void Function(Pointer<Utf8>);
typedef _StrFnC = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _StrFn = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _TwoStrFnC = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _TwoStrFn = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _StrBoolFnC = Pointer<Utf8> Function(Pointer<Utf8>, Uint8);
typedef _StrBoolFn = Pointer<Utf8> Function(Pointer<Utf8>, int);
typedef _StartC = Pointer<Utf8> Function(Pointer<Utf8>, Uint32, Uint32);
typedef _Start = Pointer<Utf8> Function(Pointer<Utf8>, int, int);
typedef _VersionC = Uint64 Function(Uint64);
typedef _Version = int Function(int);
typedef _SnapC = Pointer<Utf8> Function(Uint64);
typedef _Snap = Pointer<Utf8> Function(int);
typedef _InputC = Int32 Function(Uint64, Pointer<Uint8>, UintPtr);
typedef _Input = int Function(int, Pointer<Uint8>, int);
typedef _ResizeC = Int32 Function(Uint64, Uint32, Uint32);
typedef _Resize = int Function(int, int, int);
typedef _CloseC = Int32 Function(Uint64);
typedef _Close = int Function(int);
typedef _IndexDeleteC = Pointer<Utf8> Function(Int64);
typedef _IndexDelete = Pointer<Utf8> Function(int);
typedef _HandlePathC = Int32 Function(Uint64, Pointer<Utf8>);
typedef _HandlePath = int Function(int, Pointer<Utf8>);
typedef _HandleTwoPathsC = Int32 Function(Uint64, Pointer<Utf8>, Pointer<Utf8>);
typedef _HandleTwoPaths = int Function(int, Pointer<Utf8>, Pointer<Utf8>);
typedef _AnswerC = Int32 Function(
  Uint64,
  Uint64,
  Int32,
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _Answer = int Function(
  int,
  int,
  int,
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);

class NativeBridgeError implements Exception {
  final String message;
  const NativeBridgeError(this.message);

  @override
  String toString() => message;
}

/// Owns the loaded library and the three step-1 entry points.
class NativeBridge {
  final DynamicLibrary _lib;

  NativeBridge._(this._lib);

  static NativeBridge? _instance;
  static bool _attempted = false;

  /// The last load failure, with the paths that were tried. Cleared on success.
  /// The UI only reports a generic message, but this makes support reports
  /// actionable without a debug build.
  static String? loadError;

  /// Loads the bridge library from the platform's bundle layout (or from
  /// `XENTERM_BRIDGE_PATH`). Returns null when it cannot be loaded so the UI can
  /// show a clear error instead of crashing at startup.
  ///
  /// Candidates are tried in order and the first loadable one wins:
  /// - Windows: next to `xenterm.exe`.
  /// - Linux: the bundle root (next to the executable) and the conventional
  ///   `lib/` directory Flutter places shared libraries in.
  /// - macOS: `Contents/Frameworks/` (where the release packaging stages the
  ///   dylib and where macOS tools expect embedded frameworks) and the
  ///   executable directory itself for hand-copied builds.
  /// The bare library name is tried last so a launcher-provided search path
  /// (LD_LIBRARY_PATH / DYLD_LIBRARY_PATH) still works.
  static NativeBridge? tryLoad() {
    if (_attempted) return _instance;
    _attempted = true;
    final override = Platform.environment['XENTERM_BRIDGE_PATH'];
    final name = _defaultName();
    final candidates = override != null && override.isNotEmpty
        ? <String>[override]
        : librarySearchPaths(Platform.resolvedExecutable, name);
    final errors = <String>[];
    for (final path in [...candidates, name]) {
      try {
        final lib = DynamicLibrary.open(path);
        _instance = NativeBridge._(lib);
        loadError = null;
        return _instance;
      } catch (error) {
        errors.add('$path: $error');
      }
    }
    _instance = null;
    loadError = 'xenterm_bridge library not found '
        '(tried ${errors.join('; ')})';
    return _instance;
  }

  static String _defaultName() {
    if (Platform.isWindows) return 'xenterm_bridge.dll';
    if (Platform.isMacOS) return 'libxenterm_bridge.dylib';
    return 'libxenterm_bridge.so';
  }

  /// Message for callers when the bridge is absent. [loadError] already carries
  /// every candidate path and the original dlopen error, so a missing file,
  /// a wrong architecture and a rejected signature are no longer collapsed into
  /// one misleading "not found next to the app" line.
  static String unavailableMessage() =>
      loadError ?? 'xenterm_bridge library not found in the app bundle';

  static void resetLoadStateForTesting() {
    _attempted = false;
    _instance = null;
    loadError = null;
  }

  /// Absolute paths where the bridge may live for [executable], in lookup
  /// order. Exposed for tests so the platform bundle layouts stay covered even
  /// though CI runs on three operating systems.
  static List<String> librarySearchPaths(
    String executable,
    String name, {
    bool? isMacOS,
    bool? isLinux,
  }) {
    final dir = File(executable).parent;
    final beside = dir.uri.resolve(name).toFilePath();
    if (isMacOS ?? Platform.isMacOS) {
      // xenterm_app.app/Contents/MacOS/<exe> -> Contents/Frameworks/<lib>
      final frameworks = dir.parent.uri.resolve('Frameworks/$name').toFilePath();
      return [frameworks, beside];
    }
    if (isLinux ?? Platform.isLinux) {
      // Flutter's Linux bundle: <bundle>/xenterm + <bundle>/lib/<lib>.
      return [beside, dir.uri.resolve('lib/$name').toFilePath()];
    }
    return [beside];
  }

  /// Linux's loader does not search the working directory for a bare library
  /// name, even when the library sits next to the executable. Resolve it from
  /// the executable's own directory so a bundle launched from anywhere works.
  static String libraryPathForExecutable(String executable, String name) =>
      File(executable).parent.uri.resolve(name).toFilePath();

  late final _PtrFn _version = _lib.lookupFunction<_PtrFnC, _PtrFn>(
    'xenterm_core_version',
  );
  late final _PtrFn _sessions = _lib.lookupFunction<_PtrFnC, _PtrFn>(
    'xenterm_sessions_json',
  );
  late final _FreeFn _free = _lib.lookupFunction<_FreeFnC, _FreeFn>(
    'xenterm_string_free',
  );
  late final _StrFn _upsert = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_session_upsert',
  );
  late final _StrFn _delete = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_session_delete',
  );
  late final _PtrFn _groups = _lib.lookupFunction<_PtrFnC, _PtrFn>(
    'xenterm_groups_json',
  );
  late final _StrFn _groupCreate = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_group_create',
  );
  late final _TwoStrFn _groupRename = _lib
      .lookupFunction<_TwoStrFnC, _TwoStrFn>('xenterm_group_rename');
  late final _StrFn _groupDelete = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_group_delete',
  );
  late final _StrBoolFn _groupCollapse = _lib
      .lookupFunction<_StrBoolFnC, _StrBoolFn>('xenterm_group_collapse');
  late final _TwoStrFn _sessionMoveGroup = _lib
      .lookupFunction<_TwoStrFnC, _TwoStrFn>('xenterm_session_move_group');
  late final _StrFn _importSshConfig = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_import_ssh_config',
  );
  late final _Start _start = _lib.lookupFunction<_StartC, _Start>(
    'xenterm_session_start',
  );
  late final _StrFn _startAdhoc = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_session_start_adhoc',
  );
  late final _Version _terminalVersion = _lib
      .lookupFunction<_VersionC, _Version>('xenterm_session_version');
  late final _Snap _snapshot = _lib.lookupFunction<_SnapC, _Snap>(
    'xenterm_session_snapshot',
  );
  late final _Input _input = _lib.lookupFunction<_InputC, _Input>(
    'xenterm_session_input',
  );
  late final _Resize _resize = _lib.lookupFunction<_ResizeC, _Resize>(
    'xenterm_session_resize',
  );
  late final _Close _close = _lib.lookupFunction<_CloseC, _Close>(
    'xenterm_session_close',
  );
  late final _Answer _answer = _lib.lookupFunction<_AnswerC, _Answer>(
    'xenterm_session_answer_prompt',
  );
  late final _PtrFn _quickCommands = _lib.lookupFunction<_PtrFnC, _PtrFn>(
    'xenterm_quick_commands_json',
  );
  late final _StrFn _quickUpsert = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_quick_command_upsert',
  );
  late final _PtrFn _knownHosts = _lib.lookupFunction<_PtrFnC, _PtrFn>(
    'xenterm_known_hosts_json',
  );
  late final _StrFn _knownHostDelete = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_known_host_delete',
  );
  late final _PtrFn _forwards = _lib.lookupFunction<_PtrFnC, _PtrFn>(
    'xenterm_forwards_json',
  );
  late final _PtrFn _runtimeTunnels = _lib.lookupFunction<_PtrFnC, _PtrFn>(
    'xenterm_runtime_tunnels_json',
  );
  late final _PtrFn _profileExport = _lib.lookupFunction<_PtrFnC, _PtrFn>(
    'xenterm_profile_export',
  );
  late final _StrFn _profileImport = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_profile_import',
  );
  late final _PtrFn _jumpChains = _lib.lookupFunction<_PtrFnC, _PtrFn>(
    'xenterm_jump_chains_json',
  );
  late final _StrFn _forwardUpsert = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_forward_upsert',
  );
  late final _StrFn _forwardDelete = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_forward_delete',
  );
  late final _PtrFn _serialPorts = _lib.lookupFunction<_PtrFnC, _PtrFn>(
    'xenterm_serial_ports_json',
  );
  late final _PtrFn _keys = _lib.lookupFunction<_PtrFnC, _PtrFn>(
    'xenterm_keys_json',
  );
  late final _StrFn _keyUpdate = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_key_update',
  );
  late final _StrFn _keyDelete = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_key_delete',
  );
  late final _PtrFn _logs = _lib.lookupFunction<_PtrFnC, _PtrFn>(
    'xenterm_logs_json',
  );
  late final _StrFn _logRead = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_log_read',
  );
  late final _StrFn _logDelete = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_log_delete',
  );
  late final _PtrFn _settings = _lib.lookupFunction<_PtrFnC, _PtrFn>(
    'xenterm_settings_json',
  );
  late final _StrFn _sftpStart = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_sftp_start',
  );
  late final _Version _sftpVersion = _lib.lookupFunction<_VersionC, _Version>(
    'xenterm_sftp_version',
  );
  late final _Snap _sftpSnapshot = _lib.lookupFunction<_SnapC, _Snap>(
    'xenterm_sftp_snapshot',
  );
  late final _HandlePath _sftpList = _lib
      .lookupFunction<_HandlePathC, _HandlePath>('xenterm_sftp_list');
  late final _Close _sftpRefresh = _lib.lookupFunction<_CloseC, _Close>(
    'xenterm_sftp_refresh',
  );
  late final _HandlePath _sftpToggleTree = _lib
      .lookupFunction<_HandlePathC, _HandlePath>('xenterm_sftp_toggle_tree');
  late final _HandleTwoPaths _sftpDownload = _lib
      .lookupFunction<_HandleTwoPathsC, _HandleTwoPaths>(
        'xenterm_sftp_download',
      );
  late final _HandlePath _sftpUpload = _lib
      .lookupFunction<_HandlePathC, _HandlePath>('xenterm_sftp_upload');
  late final _HandlePath _sftpMkdir = _lib
      .lookupFunction<_HandlePathC, _HandlePath>('xenterm_sftp_mkdir');
  late final _HandlePath _sftpDelete = _lib
      .lookupFunction<_HandlePathC, _HandlePath>('xenterm_sftp_delete');
  late final _Close _sftpClose = _lib.lookupFunction<_CloseC, _Close>(
    'xenterm_sftp_close',
  );
  late final _Answer _sftpAnswer = _lib.lookupFunction<_AnswerC, _Answer>(
    'xenterm_sftp_answer_prompt',
  );
  late final _StrFn _settingsSet = _lib.lookupFunction<_StrFnC, _StrFn>(
    'xenterm_settings_set',
  );
  late final _IndexDelete _quickDelete = _lib
      .lookupFunction<_IndexDeleteC, _IndexDelete>(
        'xenterm_quick_command_delete',
      );

  String _take(Pointer<Utf8> pointer) {
    try {
      return pointer.toDartString();
    } finally {
      _free(pointer);
    }
  }

  /// The core's version string (版本号规范 v1: `xt-…`).
  String version() => _take(_version());

  /// The saved sessions as JSON, or null when the core failed to load the
  /// store (the caller surfaces an error).
  String? sessionsJson() {
    final pointer = _sessions();
    if (pointer == nullptr) return null;
    return _take(pointer);
  }

  /// Calls one string-in/string-out core entry point and releases the request.
  String _callWith(_StrFn fn, String payload) {
    final pointer = payload.toNativeUtf8();
    try {
      return _take(fn(pointer));
    } finally {
      malloc.free(pointer);
    }
  }

  Map<String, dynamic> upsertSession(Map<String, dynamic> payload) =>
      jsonDecode(_callWith(_upsert, jsonEncode(payload)))
          as Map<String, dynamic>;

  Map<String, dynamic> deleteSession(String id) =>
      jsonDecode(_callWith(_delete, id)) as Map<String, dynamic>;

  // ── Host groups ────────────────────────────────────────────────────────

  /// Named host groups as JSON (`name` + persisted `collapsed`), or null.
  String? groupsJson() {
    final pointer = _groups();
    if (pointer == nullptr) return null;
    return _take(pointer);
  }

  Map<String, dynamic> createGroup(String name) =>
      jsonDecode(_callWith(_groupCreate, name)) as Map<String, dynamic>;

  Map<String, dynamic> renameGroup(String old, String next) {
    final first = old.toNativeUtf8();
    final second = next.toNativeUtf8();
    try {
      return jsonDecode(_take(_groupRename(first, second)))
          as Map<String, dynamic>;
    } finally {
      malloc.free(first);
      malloc.free(second);
    }
  }

  Map<String, dynamic> deleteGroup(String name) =>
      jsonDecode(_callWith(_groupDelete, name)) as Map<String, dynamic>;

  Map<String, dynamic> collapseGroup(String name, bool collapsed) {
    final pointer = name.toNativeUtf8();
    try {
      return jsonDecode(_take(_groupCollapse(pointer, collapsed ? 1 : 0)))
          as Map<String, dynamic>;
    } finally {
      malloc.free(pointer);
    }
  }

  Map<String, dynamic> moveSessionToGroup(String id, String group) {
    final first = id.toNativeUtf8();
    final second = group.toNativeUtf8();
    try {
      return jsonDecode(_take(_sessionMoveGroup(first, second)))
          as Map<String, dynamic>;
    } finally {
      malloc.free(first);
      malloc.free(second);
    }
  }

  /// Imports OpenSSH config text; empty text reads `~/.ssh/config`.
  Map<String, dynamic> importSshConfig(String text) =>
      jsonDecode(_callWith(_importSshConfig, jsonEncode({'text': text})))
          as Map<String, dynamic>;

  /// Live tunnels across all open sessions (port-forward runtime state).
  String runtimeTunnelsJson() => _take(_runtimeTunnels());

  /// Portable profile export (the CLI's JSON, secrets export-key obfuscated).
  Map<String, dynamic> profileExport() =>
      jsonDecode(_take(_profileExport())) as Map<String, dynamic>;

  /// Imports a portable profile. `dryRun` validates without touching the
  /// store; applying is append-only (existing sessions are never rewritten).
  Map<String, dynamic> profileImport(String json, bool dryRun) =>
      jsonDecode(
        _callWith(
          _profileImport,
          jsonEncode({'json': json, 'dry_run': dryRun}),
        ),
      )
          as Map<String, dynamic>;

  // ── Terminal sessions ──────────────────────────────────────────────────

  /// Starts a saved session; returns its bridge handle or throws.
  int startSession(String id, int cols, int rows) {
    final pointer = id.toNativeUtf8();
    try {
      final raw = _take(_start(pointer, cols, rows));
      final result = jsonDecode(raw) as Map<String, dynamic>;
      if (result['ok'] != true) {
        throw NativeBridgeError(
          (result['error'] as String?) ?? 'Could not start the session',
        );
      }
      return (result['handle'] as num).toInt();
    } finally {
      malloc.free(pointer);
    }
  }

  int sessionVersion(int handle) => _terminalVersion(handle);

  String? sessionSnapshot(int handle) {
    final pointer = _snapshot(handle);
    if (pointer == nullptr) return null;
    return _take(pointer);
  }

  bool sendInput(int handle, Uint8List bytes) {
    if (bytes.isEmpty) return true;
    final pointer = malloc.allocate<Uint8>(bytes.length);
    try {
      pointer.asTypedList(bytes.length).setAll(0, bytes);
      return _input(handle, pointer, bytes.length) == 1;
    } finally {
      malloc.free(pointer);
    }
  }

  bool resizeSession(int handle, int cols, int rows) =>
      _resize(handle, cols, rows) == 1;

  bool closeSession(int handle) => _close(handle) == 1;

  // ── SFTP ──────────────────────────────────────────────────────────────

  int startSftp(String id) {
    final pointer = id.toNativeUtf8();
    try {
      final result =
          jsonDecode(_take(_sftpStart(pointer))) as Map<String, dynamic>;
      if (result['ok'] != true) {
        throw NativeBridgeError(
          (result['error'] as String?) ?? 'Could not open SFTP',
        );
      }
      return (result['handle'] as num).toInt();
    } finally {
      malloc.free(pointer);
    }
  }

  int sftpVersion(int handle) => _sftpVersion(handle);

  String? sftpSnapshot(int handle) {
    final pointer = _sftpSnapshot(handle);
    if (pointer == nullptr) return null;
    return _take(pointer);
  }

  bool sftpList(int handle, String path) => _withPath(_sftpList, handle, path);

  bool sftpRefresh(int handle) => _sftpRefresh(handle) == 1;

  bool sftpToggleTree(int handle, String path) =>
      _withPath(_sftpToggleTree, handle, path);

  bool sftpDownload(int handle, String remote, String localDir) {
    final remotePtr = remote.toNativeUtf8();
    final dirPtr = localDir.toNativeUtf8();
    try {
      return _sftpDownload(handle, remotePtr, dirPtr) == 1;
    } finally {
      malloc.free(remotePtr);
      malloc.free(dirPtr);
    }
  }

  bool sftpUpload(int handle, String local) =>
      _withPath(_sftpUpload, handle, local);

  bool sftpMkdir(int handle, String name) =>
      _withPath(_sftpMkdir, handle, name);

  bool sftpDelete(int handle, String path) =>
      _withPath(_sftpDelete, handle, path);

  bool sftpClose(int handle) => _sftpClose(handle) == 1;

  bool answerSftpPrompt(
    int handle,
    int promptId, {
    required bool accept,
    String? username,
    String? password,
    String? text,
  }) {
    final userPtr = username?.toNativeUtf8() ?? nullptr;
    final passPtr = password?.toNativeUtf8() ?? nullptr;
    final textPtr = text?.toNativeUtf8() ?? nullptr;
    try {
      return _sftpAnswer(
            handle,
            promptId,
            accept ? 1 : 0,
            userPtr,
            passPtr,
            textPtr,
          ) ==
          1;
    } finally {
      if (userPtr != nullptr) malloc.free(userPtr);
      if (passPtr != nullptr) malloc.free(passPtr);
      if (textPtr != nullptr) malloc.free(textPtr);
    }
  }

  bool _withPath(_HandlePath fn, int handle, String value) {
    final pointer = value.toNativeUtf8();
    try {
      return fn(handle, pointer) == 1;
    } finally {
      malloc.free(pointer);
    }
  }

  // ── Session logs & settings ───────────────────────────────────────────

  String? logsJson() {
    final pointer = _logs();
    if (pointer == nullptr) return null;
    return _take(pointer);
  }

  /// Starts a temporary session that is not written to the store.
  int startAdhocSession(Map<String, dynamic> payload) {
    final raw = _callWith(_startAdhoc, jsonEncode(payload));
    final result = jsonDecode(raw) as Map<String, dynamic>;
    if (result['ok'] != true) {
      throw NativeBridgeError(
        (result['error'] as String?) ?? 'Could not start the session',
      );
    }
    return (result['handle'] as num).toInt();
  }

  Map<String, dynamic> readLog(String name) =>
      jsonDecode(_callWith(_logRead, name)) as Map<String, dynamic>;

  Map<String, dynamic> deleteLog(String name) =>
      jsonDecode(_callWith(_logDelete, name)) as Map<String, dynamic>;

  String? settingsJson() {
    final pointer = _settings();
    if (pointer == nullptr) return null;
    return _take(pointer);
  }

  Map<String, dynamic> setSettings(Map<String, dynamic> values) =>
      jsonDecode(_callWith(_settingsSet, jsonEncode(values)))
          as Map<String, dynamic>;

  // ── Serial ports ───────────────────────────────────────────────────────

  String? serialPortsJson() {
    final pointer = _serialPorts();
    if (pointer == nullptr) return null;
    return _take(pointer);
  }

  // ── SSH keys ──────────────────────────────────────────────────────────

  String? keysJson() {
    final pointer = _keys();
    if (pointer == nullptr) return null;
    return _take(pointer);
  }

  Map<String, dynamic> updateKey(String oldPath, String newPath) => jsonDecode(
    _callWith(
      _keyUpdate,
      jsonEncode({'old_path': oldPath, 'new_path': newPath}),
    ),
  ) as Map<String, dynamic>;

  Map<String, dynamic> deleteKey(String path) =>
      jsonDecode(_callWith(_keyDelete, path)) as Map<String, dynamic>;

  // ── Port forwarding ───────────────────────────────────────────────────

  String? forwardsJson() {
    final pointer = _forwards();
    if (pointer == nullptr) return null;
    return _take(pointer);
  }

  String? jumpChainsJson() {
    final pointer = _jumpChains();
    if (pointer == nullptr) return null;
    return _take(pointer);
  }

  Map<String, dynamic> upsertForward(Map<String, dynamic> payload) =>
      jsonDecode(_callWith(_forwardUpsert, jsonEncode(payload)))
          as Map<String, dynamic>;

  Map<String, dynamic> deleteForward(Map<String, dynamic> payload) =>
      jsonDecode(_callWith(_forwardDelete, jsonEncode(payload)))
          as Map<String, dynamic>;

  // ── Known hosts ───────────────────────────────────────────────────────

  String? knownHostsJson() {
    final pointer = _knownHosts();
    if (pointer == nullptr) return null;
    return _take(pointer);
  }

  Map<String, dynamic> deleteKnownHost(String id) =>
      jsonDecode(_callWith(_knownHostDelete, id)) as Map<String, dynamic>;

  // ── Snippets (quick commands) ──────────────────────────────────────────

  String? quickCommandsJson() {
    final pointer = _quickCommands();
    if (pointer == nullptr) return null;
    return _take(pointer);
  }

  Map<String, dynamic> upsertQuickCommand(Map<String, dynamic> payload) =>
      jsonDecode(_callWith(_quickUpsert, jsonEncode(payload)))
          as Map<String, dynamic>;

  Map<String, dynamic> deleteQuickCommand(int index) =>
      jsonDecode(_take(_quickDelete(index))) as Map<String, dynamic>;

  /// Answers the current prompt: host-key uses [accept], credentials use
  /// [username]/[password], MFA uses [text].
  bool answerPrompt(
    int handle,
    int promptId, {
    required bool accept,
    String? username,
    String? password,
    String? text,
  }) {
    final userPtr = username?.toNativeUtf8() ?? nullptr;
    final passPtr = password?.toNativeUtf8() ?? nullptr;
    final textPtr = text?.toNativeUtf8() ?? nullptr;
    try {
      return _answer(
            handle,
            promptId,
            accept ? 1 : 0,
            userPtr,
            passPtr,
            textPtr,
          ) ==
          1;
    } finally {
      if (userPtr != nullptr) malloc.free(userPtr);
      if (passPtr != nullptr) malloc.free(passPtr);
      if (textPtr != nullptr) malloc.free(textPtr);
    }
  }
}

/// Result of a core write: `ok`, the affected session id, or the error text.
class CoreWriteResult {
  final bool ok;
  final String? id;
  final String? error;

  const CoreWriteResult({required this.ok, this.id, this.error});
}

/// One saved quick command (Snippets screen).
class QuickCommand {
  final int index;
  final String name;
  final String command;
  final String group;
  final bool sendEnter;

  const QuickCommand({
    required this.index,
    required this.name,
    required this.command,
    this.group = '',
    this.sendEnter = true,
  });

  factory QuickCommand.fromCoreJson(Map<String, dynamic> json) => QuickCommand(
    index: (json['index'] as num?)?.toInt() ?? -1,
    name: (json['name'] as String?) ?? '',
    command: (json['command'] as String?) ?? '',
    group: (json['group'] as String?) ?? '',
    sendEnter: json['send_enter'] as bool? ?? true,
  );
}

/// Reads the saved quick commands from the core off the UI thread.
Future<List<QuickCommand>> loadQuickCommandsFromCore() {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final raw = bridge.quickCommandsJson();
    if (raw == null) {
      throw const NativeBridgeError(
        'The core could not load the quick commands',
      );
    }
    return (jsonDecode(raw) as List<dynamic>)
        .cast<Map<String, dynamic>>()
        .map(QuickCommand.fromCoreJson)
        .toList();
  });
}

/// Creates ([index] < 0) or updates one quick command in the core.
Future<CoreWriteResult> saveQuickCommandToCore({
  required int index,
  required String name,
  required String command,
  required String group,
  required bool sendEnter,
}) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.upsertQuickCommand({
      'index': index,
      'name': name,
      'command': command,
      'group': group,
      'send_enter': sendEnter,
    });
    if (result['ok'] != true) {
      return CoreWriteResult(
        ok: false,
        error: (result['error'] as String?) ?? 'Could not save the snippet',
      );
    }
    return CoreWriteResult(
      ok: true,
      id: (result['index'] as num?)?.toInt().toString(),
    );
  });
}

/// Deletes one quick command by its current index.
Future<CoreWriteResult> deleteQuickCommandFromCore(int index) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.deleteQuickCommand(index);
    if (result['ok'] != true) {
      return CoreWriteResult(
        ok: false,
        error: (result['error'] as String?) ?? 'Could not delete the snippet',
      );
    }
    return const CoreWriteResult(ok: true);
  });
}

/// One remote SFTP directory entry.
class SftpEntry {
  final String name;
  final String path;
  final bool isDir;
  final int size;
  final int modified;
  final int mode;

  const SftpEntry({
    required this.name,
    required this.path,
    required this.isDir,
    required this.size,
    required this.modified,
    required this.mode,
  });

  factory SftpEntry.fromCoreJson(Map<String, dynamic> json) => SftpEntry(
    name: (json['name'] as String?) ?? '',
    path: (json['path'] as String?) ?? '',
    isDir: json['dir'] as bool? ?? false,
    size: (json['size'] as num?)?.toInt() ?? 0,
    modified: (json['modified'] as num?)?.toInt() ?? 0,
    mode: (json['mode'] as num?)?.toInt() ?? 0,
  );

  String get permissions {
    if (mode == 0) return '';
    const chars = ['---', '--x', '-w-', '-wx', 'r--', 'r-x', 'rw-', 'rwx'];
    return [
      for (var shift = 6; shift >= 0; shift -= 3) chars[(mode >> shift) & 7],
    ].join();
  }
}

/// One folder in the remote tree (the root is "/").
class SftpTreeNode {
  final String path;
  final String name;
  final int depth;
  final bool expanded;
  final bool hasChildren;

  const SftpTreeNode({
    required this.path,
    required this.name,
    required this.depth,
    required this.expanded,
    required this.hasChildren,
  });

  factory SftpTreeNode.fromCoreJson(Map<String, dynamic> json) => SftpTreeNode(
    path: (json['path'] as String?) ?? '',
    name: (json['name'] as String?) ?? '',
    depth: (json['depth'] as num?)?.toInt() ?? 0,
    expanded: json['expanded'] as bool? ?? false,
    hasChildren: json['has_children'] as bool? ?? false,
  );
}

class SftpTransfer {
  final String id;
  final String name;
  final bool upload;
  final int transferred;
  final int total;
  final int state;
  final String message;

  const SftpTransfer({
    required this.id,
    required this.name,
    required this.upload,
    required this.transferred,
    required this.total,
    required this.state,
    required this.message,
  });

  factory SftpTransfer.fromCoreJson(Map<String, dynamic> json) => SftpTransfer(
    id: (json['id'] as String?) ?? '',
    name: (json['name'] as String?) ?? '',
    upload: json['upload'] as bool? ?? false,
    transferred: (json['transferred'] as num?)?.toInt() ?? 0,
    total: (json['total'] as num?)?.toInt() ?? 0,
    state: (json['state'] as num?)?.toInt() ?? 0,
    message: (json['msg'] as String?) ?? '',
  );
}

/// A polled SFTP browser snapshot.
class SftpSnapshot {
  final String path;
  final List<SftpEntry> entries;
  final bool loading;
  final String? error;
  final String status;
  final String? closed;
  final List<SftpTransfer> transfers;
  final Map<String, dynamic>? prompt;
  final List<SftpTreeNode> tree;

  const SftpSnapshot({
    this.path = '.',
    this.entries = const [],
    this.loading = false,
    this.error,
    this.status = '',
    this.closed,
    this.transfers = const [],
    this.prompt,
    this.tree = const [],
  });

  factory SftpSnapshot.fromCoreJson(Map<String, dynamic> json) => SftpSnapshot(
    path: (json['path'] as String?) ?? '.',
    entries: ((json['entries'] as List?) ?? const [])
        .cast<Map<String, dynamic>>()
        .map(SftpEntry.fromCoreJson)
        .toList(),
    loading: json['loading'] as bool? ?? false,
    error: json['error'] as String?,
    status: (json['status'] as String?) ?? '',
    closed: json['closed'] as String?,
    transfers: ((json['transfers'] as List?) ?? const [])
        .cast<Map<String, dynamic>>()
        .map(SftpTransfer.fromCoreJson)
        .toList(),
    prompt: (json['prompt'] as Map?)?.cast<String, dynamic>(),
    tree: ((json['tree'] as List?) ?? const [])
        .cast<Map<String, dynamic>>()
        .map(SftpTreeNode.fromCoreJson)
        .toList(),
  );
}

/// One saved terminal transcript.
class SessionLogEntry {
  final String file;
  final String sessionId;
  final String name;
  final String host;
  final String user;
  final int startedMs;
  final int size;

  const SessionLogEntry({
    required this.file,
    required this.sessionId,
    required this.name,
    required this.host,
    required this.user,
    required this.startedMs,
    required this.size,
  });

  factory SessionLogEntry.fromCoreJson(Map<String, dynamic> json) =>
      SessionLogEntry(
        file: (json['file'] as String?) ?? '',
        sessionId: (json['session_id'] as String?) ?? '',
        name: (json['name'] as String?) ?? '',
        host: (json['host'] as String?) ?? '',
        user: (json['user'] as String?) ?? '',
        startedMs: (json['started_ms'] as num?)?.toInt() ?? 0,
        size: (json['size'] as num?)?.toInt() ?? 0,
      );

  DateTime get started =>
      DateTime.fromMillisecondsSinceEpoch(startedMs, isUtc: true).toLocal();
}

/// A transcript's content plus its metadata.
class SessionLogContent {
  final String text;
  final Map<String, dynamic> meta;
  final int? endedMs;
  final String? closeReason;

  const SessionLogContent({
    required this.text,
    required this.meta,
    this.endedMs,
    this.closeReason,
  });
}

Future<List<SessionLogEntry>> loadSessionLogsFromCore() {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final raw = bridge.logsJson();
    if (raw == null) {
      throw const NativeBridgeError('The core could not list session logs');
    }
    return (jsonDecode(raw) as List<dynamic>)
        .cast<Map<String, dynamic>>()
        .map(SessionLogEntry.fromCoreJson)
        .toList();
  });
}

Future<SessionLogContent> readSessionLogFromCore(String file) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.readLog(file);
    if (result['ok'] != true) {
      throw NativeBridgeError(
        (result['error'] as String?) ?? 'Could not read the transcript',
      );
    }
    return SessionLogContent(
      text: (result['text'] as String?) ?? '',
      meta:
          (result['meta'] as Map?)?.cast<String, dynamic>() ??
          const <String, dynamic>{},
      endedMs: (result['ended_ms'] as num?)?.toInt(),
      closeReason: result['close_reason'] as String?,
    );
  });
}

Future<CoreWriteResult> deleteSessionLogFromCore(String file) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.deleteLog(file);
    if (result['ok'] != true) {
      return CoreWriteResult(
        ok: false,
        error: (result['error'] as String?) ?? 'Could not delete the log',
      );
    }
    return const CoreWriteResult(ok: true);
  });
}

/// Real core settings the Flutter UI can change.
class CoreSettings {
  final bool sessionLogs;
  final String cursorStyle;
  final double lineSpacing;
  final bool padding;
  final bool bold;
  final bool sftpFollowCd;
  final String downloadDir;

  const CoreSettings({
    this.sessionLogs = false,
    this.cursorStyle = 'block',
    this.lineSpacing = 1.0,
    this.padding = true,
    this.bold = false,
    this.sftpFollowCd = true,
    this.downloadDir = '',
  });

  factory CoreSettings.fromCoreJson(Map<String, dynamic> json) => CoreSettings(
    sessionLogs: json['session_logs'] as bool? ?? false,
    cursorStyle: (json['cursor_style'] as String?) ?? 'block',
    lineSpacing: (json['line_spacing'] as num?)?.toDouble() ?? 1.0,
    padding: json['padding'] as bool? ?? true,
    bold: json['bold'] as bool? ?? false,
    sftpFollowCd: json['sftp_follow_cd'] as bool? ?? true,
    downloadDir: (json['download_dir'] as String?) ?? '',
  );
}

Future<CoreSettings> loadSettingsFromCore() {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final raw = bridge.settingsJson();
    if (raw == null) {
      throw const NativeBridgeError('The core could not load settings');
    }
    return CoreSettings.fromCoreJson(
      (jsonDecode(raw) as Map).cast<String, dynamic>(),
    );
  });
}

Future<CoreWriteResult> saveSettingsToCore(Map<String, dynamic> values) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.setSettings(values);
    if (result['ok'] != true) {
      return CoreWriteResult(
        ok: false,
        error: (result['error'] as String?) ?? 'Could not save settings',
      );
    }
    return const CoreWriteResult(ok: true);
  });
}

/// One SSH key credential referenced by a saved session. Key material never
/// crosses the bridge — only its path, algorithm, fingerprint and usage.
class KeyInfo {
  final String sessionId;
  final String sessionName;
  final String host;
  final String user;
  final String source; // file | inline
  final String path; // file name only
  final String fullPath; // configured private_key_path
  final String keyType;
  final String fingerprint;
  final String status; // available | locked | missing

  const KeyInfo({
    required this.sessionId,
    required this.sessionName,
    required this.host,
    required this.user,
    required this.source,
    required this.path,
    required this.fullPath,
    required this.keyType,
    required this.fingerprint,
    required this.status,
  });

  factory KeyInfo.fromCoreJson(Map<String, dynamic> json) => KeyInfo(
    sessionId: (json['session_id'] as String?) ?? '',
    sessionName: (json['session_name'] as String?) ?? '',
    host: (json['host'] as String?) ?? '',
    user: (json['user'] as String?) ?? '',
    source: (json['source'] as String?) ?? 'file',
    path: (json['path'] as String?) ?? '',
    fullPath: (json['full_path'] as String?) ?? '',
    keyType: (json['key_type'] as String?) ?? '',
    fingerprint: (json['fingerprint'] as String?) ?? '',
    status: (json['status'] as String?) ?? 'available',
  );

  String get title =>
      source == 'inline' ? 'Pasted key' : (path.isEmpty ? 'Key file' : path);

  String get usage => user.isEmpty ? host : '$user@$host';
}

Future<List<KeyInfo>> loadKeysFromCore() {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final raw = bridge.keysJson();
    if (raw == null) {
      throw const NativeBridgeError('The core could not load keys');
    }
    return (jsonDecode(raw) as List<dynamic>)
        .cast<Map<String, dynamic>>()
        .map(KeyInfo.fromCoreJson)
        .toList();
  });
}

Future<CoreWriteResult> updateKeyInCore({
  required String oldPath,
  required String newPath,
}) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.updateKey(oldPath, newPath);
    return CoreWriteResult(
      ok: result['ok'] == true,
      error: result['error'] as String?,
    );
  });
}

Future<CoreWriteResult> deleteKeyFromCore(String path) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.deleteKey(path);
    return CoreWriteResult(
      ok: result['ok'] == true,
      error: result['error'] as String?,
    );
  });
}

/// One saved port-forward rule and the session that owns it.
class ForwardRule {
  final String sessionId;
  final String sessionName;
  final String sessionKind;
  final int index;
  final String kind;
  final String name;
  final String bindAddr;
  final int bindPort;
  final String host;
  final int hostPort;

  const ForwardRule({
    required this.sessionId,
    required this.sessionName,
    required this.sessionKind,
    required this.index,
    required this.kind,
    required this.name,
    required this.bindAddr,
    required this.bindPort,
    required this.host,
    required this.hostPort,
  });

  factory ForwardRule.fromCoreJson(Map<String, dynamic> json) => ForwardRule(
    sessionId: (json['session_id'] as String?) ?? '',
    sessionName: (json['session_name'] as String?) ?? '',
    sessionKind: (json['session_kind'] as String?) ?? 'ssh',
    index: (json['index'] as num?)?.toInt() ?? 0,
    kind: (json['kind'] as String?) ?? 'local',
    name: (json['name'] as String?) ?? '',
    bindAddr: (json['bind_addr'] as String?) ?? '',
    bindPort: (json['bind_port'] as num?)?.toInt() ?? 0,
    host: (json['host'] as String?) ?? '',
    hostPort: (json['host_port'] as num?)?.toInt() ?? 0,
  );

  String get bind => '$bindAddr:$bindPort';
  String get target => kind == 'dynamic'
      ? 'SOCKS5'
      : host.isEmpty || hostPort == 0
      ? ''
      : '$host:$hostPort';
}

Future<List<ForwardRule>> loadForwardsFromCore() {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final raw = bridge.forwardsJson();
    if (raw == null) {
      throw const NativeBridgeError('The core could not load port forwards');
    }
    final decoded = jsonDecode(raw);
    if (decoded is Map && decoded['__error'] != null) {
      throw NativeBridgeError('${decoded['__error']}');
    }
    return (decoded as List<dynamic>)
        .cast<Map<String, dynamic>>()
        .map(ForwardRule.fromCoreJson)
        .toList();
  });
}

/// One saved session reached through jump hosts.
class JumpChain {
  final String sessionId;
  final String sessionName;
  final String host;
  final List<String> jumps;

  const JumpChain({
    required this.sessionId,
    required this.sessionName,
    required this.host,
    required this.jumps,
  });

  factory JumpChain.fromCoreJson(Map<String, dynamic> json) => JumpChain(
    sessionId: (json['session_id'] as String?) ?? '',
    sessionName: (json['session_name'] as String?) ?? '',
    host: (json['host'] as String?) ?? '',
    jumps: ((json['jumps'] as List?) ?? const [])
        .map((value) => '$value')
        .toList(),
  );

  String get path => [...jumps, sessionName].join('  →  ');
}

Future<List<JumpChain>> loadJumpChainsFromCore() {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final raw = bridge.jumpChainsJson();
    if (raw == null) {
      throw const NativeBridgeError('The core could not load jump chains');
    }
    final decoded = jsonDecode(raw);
    if (decoded is Map && decoded['__error'] != null) {
      throw NativeBridgeError('${decoded['__error']}');
    }
    return (decoded as List<dynamic>)
        .cast<Map<String, dynamic>>()
        .map(JumpChain.fromCoreJson)
        .toList();
  });
}

Future<CoreWriteResult> saveForwardToCore({
  required String sessionId,
  required int index,
  required String kind,
  required String name,
  required String bindAddr,
  required String bindPort,
  required String host,
  required String hostPort,
}) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.upsertForward({
      'session_id': sessionId,
      'index': index,
      'kind': kind,
      'name': name,
      'bind_addr': bindAddr,
      'bind_port': bindPort,
      'host': host,
      'host_port': hostPort,
    });
    if (result['ok'] != true) {
      return CoreWriteResult(
        ok: false,
        error: (result['error'] as String?) ?? 'Could not save the rule',
      );
    }
    return CoreWriteResult(
      ok: true,
      id: (result['index'] as num?)?.toInt().toString(),
    );
  });
}

Future<CoreWriteResult> deleteForwardFromCore({
  required String sessionId,
  required int index,
}) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.deleteForward({
      'session_id': sessionId,
      'index': index,
    });
    if (result['ok'] != true) {
      return CoreWriteResult(
        ok: false,
        error: (result['error'] as String?) ?? 'Could not delete the rule',
      );
    }
    return const CoreWriteResult(ok: true);
  });
}

/// One remembered SSH host key (`known_hosts`, TOFU).
class KnownHost {
  final String id;
  final String keyType;
  final String fingerprint;

  const KnownHost({
    required this.id,
    required this.keyType,
    required this.fingerprint,
  });

  factory KnownHost.fromCoreJson(Map<String, dynamic> json) => KnownHost(
    id: (json['id'] as String?) ?? '',
    keyType: (json['key_type'] as String?) ?? '',
    fingerprint: (json['fingerprint'] as String?) ?? '',
  );

  String get host {
    final at = id.lastIndexOf(':');
    return at > 0 ? id.substring(0, at) : id;
  }

  String get port =>
      id.lastIndexOf(':') > 0 ? id.substring(id.lastIndexOf(':') + 1) : '';
}

/// Reads the remembered host keys from the core off the UI thread.
Future<List<KnownHost>> loadKnownHostsFromCore() {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final raw = bridge.knownHostsJson();
    if (raw == null) {
      throw const NativeBridgeError('The core could not load known hosts');
    }
    return (jsonDecode(raw) as List<dynamic>)
        .cast<Map<String, dynamic>>()
        .map(KnownHost.fromCoreJson)
        .toList();
  });
}

/// Forgets one `host:port` entry in the core's TOFU store.
Future<CoreWriteResult> deleteKnownHostFromCore(String id) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.deleteKnownHost(id);
    if (result['ok'] != true) {
      return CoreWriteResult(
        ok: false,
        error: (result['error'] as String?) ?? 'Could not remove the host key',
      );
    }
    return CoreWriteResult(ok: true, id: id);
  });
}

/// Loads the saved sessions from the core off the UI thread and maps them to
/// the hosts-screen model. Throws [NativeBridgeError] with a user-facing
/// message when the bridge or the store is unavailable.
Future<List<Host>> loadHostsFromCore() {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final raw = bridge.sessionsJson();
    if (raw == null) {
      throw const NativeBridgeError(
        'The core could not load the session store',
      );
    }
    final rows = (jsonDecode(raw) as List<dynamic>)
        .cast<Map<String, dynamic>>();
    return rows.map(Host.fromCoreJson).toList();
  });
}

/// One serial device reported by the OS.
class SerialPortInfo {
  final String name;
  final String kind;

  const SerialPortInfo({required this.name, required this.kind});

  factory SerialPortInfo.fromCoreJson(Map<String, dynamic> json) =>
      SerialPortInfo(
        name: (json['name'] as String?) ?? '',
        kind: (json['kind'] as String?) ?? 'Serial',
      );

  String get label => kind.isEmpty ? name : '$name · $kind';
}

/// Serial ports currently attached to this machine.
Future<List<SerialPortInfo>> loadSerialPortsFromCore() {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final raw = bridge.serialPortsJson();
    if (raw == null) {
      throw const NativeBridgeError('The core could not list serial ports');
    }
    return (jsonDecode(raw) as List<dynamic>)
        .cast<Map<String, dynamic>>()
        .map(SerialPortInfo.fromCoreJson)
        .toList();
  });
}

/// Creates or updates a session in the core's store. Ids are generated by the
/// core; an empty id creates. Runs off the UI thread (store save touches the
/// keyring-protected SQLite file).
Future<CoreWriteResult> saveSessionToCore({
  required String id,
  required String label,
  required String address,
  required int port,
  required String username,
  required String group,
  required String kind,
  String keyPath = '',
  String keyInline = '',
  bool clearKey = false,
  int baudRate = 115200,
  int dataBits = 8,
  int stopBits = 1,
  String parity = 'none',
  String flowControl = 'none',
  String password = '',
  bool clearPassword = false,
  String jumpHostId = '',
  String encoding = '',
  List<Map<String, dynamic>> triggers = const [],
}) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.upsertSession({
      'id': id,
      'label': label,
      'address': address,
      'port': port,
      'username': username,
      'group': group,
      'kind': kind,
      'private_key_path': keyPath,
      'private_key_inline': keyInline,
      'clear_private_key': clearKey,
      'encoding': encoding,
      'baud_rate': baudRate,
      'data_bits': dataBits,
      'stop_bits': stopBits,
      'parity': parity,
      'flow_control': flowControl,
      'password': password,
      'clear_password': clearPassword,
      'jump_session_id': jumpHostId,
      if (triggers.isNotEmpty) 'triggers': triggers,
    });
    return CoreWriteResult(
      ok: result['ok'] == true,
      id: result['id'] as String?,
      error: result['error'] as String?,
    );
  });
}

/// Deletes one saved session from the core's store.
Future<CoreWriteResult> deleteSessionFromCore(String id) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.deleteSession(id);
    return CoreWriteResult(
      ok: result['ok'] == true,
      error: result['error'] as String?,
    );
  });
}

/// One live port-forward tunnel reported by an open session.
class RuntimeTunnel {
  final String id;
  final String name;
  final String kind;
  final String bindAddr;
  final int bindPort;
  final String host;
  final int hostPort;
  final bool active;
  final String status;
  final int sessionHandle;

  const RuntimeTunnel({
    required this.id,
    required this.name,
    required this.kind,
    required this.bindAddr,
    required this.bindPort,
    required this.host,
    required this.hostPort,
    required this.active,
    required this.status,
    required this.sessionHandle,
  });

  factory RuntimeTunnel.fromCoreJson(Map<String, dynamic> json) =>
      RuntimeTunnel(
        id: (json['id'] as String?) ?? '',
        name: (json['name'] as String?) ?? '',
        kind: (json['kind'] as String?) ?? '',
        bindAddr: (json['bindAddr'] as String?) ?? '',
        bindPort: (json['bindPort'] as num?)?.toInt() ?? 0,
        host: (json['host'] as String?) ?? '',
        hostPort: (json['hostPort'] as num?)?.toInt() ?? 0,
        active: json['active'] == true,
        status: (json['status'] as String?) ?? '',
        sessionHandle: (json['session_handle'] as num?)?.toInt() ?? 0,
      );
}

/// Tunnels currently open by the running sessions (empty when none).
Future<List<RuntimeTunnel>> loadRuntimeTunnelsFromCore() {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) return const <RuntimeTunnel>[];
    final rows = jsonDecode(bridge.runtimeTunnelsJson()) as List<dynamic>;
    return rows
        .cast<Map<String, dynamic>>()
        .map(RuntimeTunnel.fromCoreJson)
        .toList();
  });
}

/// The portable profile export JSON and its session count.
class ProfileExport {
  final String json;
  final int count;

  const ProfileExport({required this.json, required this.count});
}

Future<ProfileExport> exportProfileFromCore() {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.profileExport();
    if (result['ok'] != true) {
      throw NativeBridgeError((result['error'] as String?) ?? 'Export failed');
    }
    return ProfileExport(
      json: (result['json'] as String?) ?? '',
      count: (result['count'] as num?)?.toInt() ?? 0,
    );
  });
}

/// One import pass result (dry-run previews share this shape).
class ProfileImportResult {
  final int added;
  final int skipped;
  final List<String> warnings;

  const ProfileImportResult({
    required this.added,
    required this.skipped,
    required this.warnings,
  });
}

/// Imports a portable profile. Preview first (`dryRun: true`), then apply.
Future<ProfileImportResult> importProfileToCore(
  String json, {
  bool dryRun = false,
}) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.profileImport(json, dryRun);
    if (result['ok'] != true) {
      throw NativeBridgeError((result['error'] as String?) ?? 'Import failed');
    }
    return ProfileImportResult(
      added: (result['added'] as num?)?.toInt() ?? 0,
      skipped: (result['skipped'] as num?)?.toInt() ?? 0,
      warnings: ((result['warnings'] as List?) ?? const [])
          .map((w) => w is Map ? (w['code']?.toString() ?? '') : w.toString())
          .toList(),
    );
  });
}

/// One named host group and its persisted open/closed state.
class HostGroup {
  final String name;
  final bool collapsed;

  const HostGroup({required this.name, required this.collapsed});

  factory HostGroup.fromCoreJson(Map<String, dynamic> json) => HostGroup(
    name: (json['name'] as String?) ?? '',
    collapsed: json['collapsed'] == true,
  );
}

/// Named host groups from the core (explicit folders ∪ groups in use).
Future<List<HostGroup>> loadGroupsFromCore() {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final raw = bridge.groupsJson();
    if (raw == null) {
      throw const NativeBridgeError('The core could not load host groups');
    }
    return (jsonDecode(raw) as List<dynamic>)
        .cast<Map<String, dynamic>>()
        .map(HostGroup.fromCoreJson)
        .toList();
  });
}

Future<CoreWriteResult> createGroupInCore(String name) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.createGroup(name);
    return CoreWriteResult(
      ok: result['ok'] == true,
      error: result['error'] as String?,
    );
  });
}

Future<CoreWriteResult> renameGroupInCore(String old, String next) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.renameGroup(old, next);
    return CoreWriteResult(
      ok: result['ok'] == true,
      error: result['error'] as String?,
    );
  });
}

Future<CoreWriteResult> deleteGroupFromCore(String name) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.deleteGroup(name);
    return CoreWriteResult(
      ok: result['ok'] == true,
      error: result['error'] as String?,
    );
  });
}

Future<CoreWriteResult> collapseGroupInCore(String name, bool collapsed) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.collapseGroup(name, collapsed);
    return CoreWriteResult(
      ok: result['ok'] == true,
      error: result['error'] as String?,
    );
  });
}

Future<CoreWriteResult> moveSessionToGroupInCore(String id, String group) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.moveSessionToGroup(id, group);
    return CoreWriteResult(
      ok: result['ok'] == true,
      error: result['error'] as String?,
    );
  });
}

/// Outcome of an OpenSSH config import.
class ImportResult {
  final bool ok;
  final int added;
  final int skipped;
  final String message;
  final String? error;

  const ImportResult({
    required this.ok,
    this.added = 0,
    this.skipped = 0,
    this.message = '',
    this.error,
  });
}

/// Imports hosts from OpenSSH config text (empty = `~/.ssh/config`).
Future<ImportResult> importSshConfigToCore({String text = ''}) {
  return Isolate.run(() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) {
      throw NativeBridgeError(NativeBridge.unavailableMessage());
    }
    final result = bridge.importSshConfig(text);
    return ImportResult(
      ok: result['ok'] == true,
      added: (result['added'] as num?)?.toInt() ?? 0,
      skipped: (result['skipped'] as num?)?.toInt() ?? 0,
      message: (result['message'] as String?) ?? '',
      error: result['error'] as String?,
    );
  });
}

/// Synchronous variant for diagnostics (e.g. the core version line).
String? coreVersionSync() => NativeBridge.tryLoad()?.version();
