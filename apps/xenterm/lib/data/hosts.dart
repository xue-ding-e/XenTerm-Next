/// Host model shared by the hosts screen; rows arrive from the Rust core via
/// the FFI bridge (lib/bridge/native_bridge.dart).
///
/// The shapes mirror the Rust side's `Session` projection the bridge will
/// expose — same field names where they carry over — so replacing the
/// repository with generated bindings touches screens, not models.
library;

/// The OS a host runs — drives the neutral OS chip on cards and in the details
/// drawer. [unknown] is the honest default: the core does not report an OS
/// until the first connection, so guessing a distro logo would be a lie.
enum HostOs { unknown, debian, ubuntu, arch, linux, windows, apple, android }

class Host {
  final String id;
  final String label;
  final String address;
  final int port;
  final String username;
  final String group;
  final List<String> tags;
  final HostOs os;
  final String? lastUsed;

  /// "ssh" / "serial" / "telnet" / "local" — the first part of the card line.
  final String protocol;

  /// "password" / "key" / "keyboard-interactive".
  final String auth;

  /// Non-secret private key path; empty when the host uses a password.
  final String keyPath;

  /// Whether imported/pasted key material is stored in the core (the material
  /// itself never leaves the store).
  final bool hasKeyInline;

  /// Terminal charset for this session (WHATWG label, e.g. "gbk"); the core
  /// falls back to UTF-8 for unknown labels.
  final String encoding;

  // Serial-line settings (kind == 'serial').
  final int baudRate;
  final int dataBits;
  final int stopBits;
  final String parity;
  final String flowControl;

  /// Whether the core has a stored login password for this serial session.
  final bool hasPassword;

  /// Another saved SSH host used as this session's jump host (empty = none).
  final String jumpHostId;

  /// Temporary session (local/serial quick connect) that must not be stored.
  final bool ephemeral;

  /// One-shot credential for a temporary serial console login. Lives only in
  /// memory; saved hosts pass the secret through the core store instead.
  final String adhocPassword;

  /// Expect/send automation rules (the core fires them during the session).
  final List<SessionTriggerRule> triggers;

  const Host({
    required this.id,
    required this.label,
    required this.address,
    this.port = 22,
    this.username = 'root',
    this.group = '',
    this.tags = const [],
    this.os = HostOs.unknown,
    this.lastUsed,
    this.protocol = 'ssh',
    this.auth = 'password',
    this.keyPath = '',
    this.hasKeyInline = false,
    this.encoding = 'utf-8',
    this.baudRate = 115200,
    this.dataBits = 8,
    this.stopBits = 1,
    this.parity = 'none',
    this.flowControl = 'none',
    this.hasPassword = false,
    this.jumpHostId = '',
    this.ephemeral = false,
    this.adhocPassword = '',
    this.triggers = const [],
  });

  /// Maps one `xenterm_sessions_json` row (native bridge) onto the UI model.
  factory Host.fromCoreJson(Map<String, dynamic> json) {
    final kind = (json['kind'] as String?) ?? 'ssh';
    final address = (json['address'] as String?) ?? '';
    final label = (json['label'] as String?) ?? '';
    return Host(
      id: (json['id'] ?? '').toString(),
      label: label.isEmpty ? address : label,
      address: address,
      port: (json['port'] as num?)?.toInt() ?? 22,
      username: (json['username'] as String?) ?? '',
      group: (json['group'] as String?) ?? '',
      protocol: kind,
      os: HostOs.unknown,
      lastUsed: json['last_used'] as String?,
      auth: (json['auth'] as String?) ?? 'password',
      keyPath: (json['private_key_path'] as String?) ?? '',
      hasKeyInline: json['has_inline_key'] == true,
      encoding: (json['encoding'] as String?)?.trim().isNotEmpty == true
          ? (json['encoding'] as String).trim()
          : 'utf-8',
      baudRate: (json['baud_rate'] as num?)?.toInt() ?? 115200,
      dataBits: (json['data_bits'] as num?)?.toInt() ?? 8,
      stopBits: (json['stop_bits'] as num?)?.toInt() ?? 1,
      parity: (json['parity'] as String?) ?? 'none',
      flowControl: (json['flow_control'] as String?) ?? 'none',
      hasPassword: json['has_password'] == true,
      jumpHostId: (json['jump_session_id'] as String?) ?? '',
      triggers: ((json['triggers'] as List?) ?? const [])
          .cast<Map<String, dynamic>>()
          .map(SessionTriggerRule.fromCoreJson)
          .toList(),
    );
  }

  /// Payload for the core's ad-hoc starter (temporary tabs).
  Map<String, dynamic> toAdhocPayload() => {
    'label': label,
    'address': address,
    'port': port,
    'username': username,
    'kind': protocol,
    'password': adhocPassword,
    'serial_port': isSerial ? address : '',
    'baud_rate': baudRate,
    'data_bits': dataBits,
    'stop_bits': stopBits,
    'parity': parity,
    'flow_control': flowControl,
  };

  Host copyWith({HostOs? os, bool? hasPassword}) => Host(
    id: id,
    label: label,
    address: address,
    port: port,
    username: username,
    group: group,
    tags: tags,
    os: os ?? this.os,
    lastUsed: lastUsed,
    protocol: protocol,
    auth: auth,
    keyPath: keyPath,
    hasKeyInline: hasKeyInline,
    encoding: encoding,
    baudRate: baudRate,
    dataBits: dataBits,
    stopBits: stopBits,
    parity: parity,
    flowControl: flowControl,
    hasPassword: hasPassword ?? this.hasPassword,
    jumpHostId: jumpHostId,
    ephemeral: ephemeral,
    adhocPassword: adhocPassword,
    triggers: triggers,
  );

  bool get isSerial => protocol == 'serial';

  /// Whether any private-key credential is stored (path or inline material).
  bool get hasKey => keyPath.isNotEmpty || hasKeyInline;

  /// "8N1" for a serial line.
  String get serialFrame {
    final parityLetter = switch (parity) {
      'odd' => 'O',
      'even' => 'E',
      _ => 'N',
    };
    return '$dataBits$parityLetter$stopBits';
  }

  String get detail => isSerial
      ? '$address · $baudRate ${serialFrame.toUpperCase()}'
      : username.isEmpty
      ? address
      : '$username@$address:$port';

  /// "ssh, root, 阿里云" — the card's second line.
  String get subtitle {
    final parts = <String>[
      protocol,
      if (isSerial)
        '$baudRate ${serialFrame.toUpperCase()}'
      else if (username.isNotEmpty)
        username,
      if (tags.isNotEmpty) ...tags else if (group.isNotEmpty) group,
    ];
    return parts.join(', ');
  }
}

/// Sample fixture used by widget tests (the app itself loads the core's real
/// sessions). OS + tags + order mirror the reference screenshot.
const kSampleHosts = <Host>[
  Host(
    id: 'sz-ali',
    label: '小白的深圳服务器（阿里云）',
    address: 'sz.example.cn',
    group: '生产',
    tags: ['阿里云'],
    os: HostOs.debian,
  ),
  Host(
    id: 'nb-yy',
    label: '小白的宁波服务器(雨云)',
    address: 'nb.example.cn',
    group: '生产',
    tags: ['雨云'],
    os: HostOs.ubuntu,
    lastUsed: '2026-10-14 09:32',
  ),
  Host(
    id: 'nat-zz',
    label: '小白的枣庄NAT服务器（小兔互联）',
    address: 'nat.example.cn',
    port: 22022,
    group: '生产',
    os: HostOs.debian,
    lastUsed: '2026-10-15 21:04',
  ),
  Host(
    id: 'gw2',
    label: 'gateway-test02',
    address: '10.0.0.2',
    group: '内网',
    os: HostOs.ubuntu,
  ),
  Host(
    id: 'nanopi',
    label: 'NanoPi-R3S（物联网小主机）',
    address: '192.168.2.10',
    username: 'pi',
    group: '内网',
    os: HostOs.ubuntu,
  ),
  Host(
    id: 'dev-box',
    label: '开发机',
    address: '10.8.0.3',
    username: 'dev',
    os: HostOs.debian,
  ),
];


/// One expect/send rule. The response text is a secret: the core only reports
/// whether one is stored, and an empty [draftResponse] keeps it on save.
class SessionTriggerRule {
  /// Output the session waits for before answering, e.g. `login:`.
  final String expect;

  /// Whether the core has a stored response for this rule.
  final bool hasResponse;

  final bool appendEnter;

  /// False = the rule is consumed after its first match.
  final bool repeat;

  /// Editor-only replacement text; empty keeps the stored response unless
  /// [clearResponse] is set.
  final String draftResponse;
  final bool clearResponse;

  const SessionTriggerRule({
    required this.expect,
    this.hasResponse = false,
    this.appendEnter = true,
    this.repeat = false,
    this.draftResponse = '',
    this.clearResponse = false,
  });

  factory SessionTriggerRule.fromCoreJson(Map<String, dynamic> json) =>
      SessionTriggerRule(
        expect: (json['expect'] as String?) ?? '',
        hasResponse: json['has_response'] == true,
        appendEnter: json['append_enter'] != false,
        repeat: json['repeat'] == true,
      );

  /// Wire format for the bridge's session upsert.
  Map<String, dynamic> toCoreJson() => {
    'expect': expect,
    'response': draftResponse,
    if (clearResponse) 'clear_response': true,
    'append_enter': appendEnter,
    'repeat': repeat,
  };

  SessionTriggerRule copyWith({
    String? expect,
    bool? hasResponse,
    bool? appendEnter,
    bool? repeat,
    String? draftResponse,
    bool? clearResponse,
  }) => SessionTriggerRule(
    expect: expect ?? this.expect,
    hasResponse: hasResponse ?? this.hasResponse,
    appendEnter: appendEnter ?? this.appendEnter,
    repeat: repeat ?? this.repeat,
    draftResponse: draftResponse ?? this.draftResponse,
    clearResponse: clearResponse ?? this.clearResponse,
  );
}

/// Draft-level validation shared by the SSH and serial editors.
///
/// Blocks empty connections (an address — or serial port — and an SSH
/// username are the minimum a session needs to be usable) and duplicate
/// names, so the store never fills with unusable "New Host" shells.
/// [editingId] excludes the host being edited from the duplicate check;
/// ephemeral quick-connect drafts are excluded entirely.
String? validateHostDraft({
  required String label,
  required String address,
  required String username,
  required String kind,
  required List<Host> hosts,
  String? editingId,
}) {
  final trimmedAddress = address.trim();
  if (trimmedAddress.isEmpty) {
    return kind == 'serial'
        ? 'A serial port is required (pick one or type a path).'
        : 'The address is required.';
  }
  if (kind != 'serial' && username.trim().isEmpty) {
    return 'The username is required.';
  }
  final name = label.trim().isEmpty ? trimmedAddress : label.trim();
  final lower = name.toLowerCase();
  for (final host in hosts) {
    if (host.id == (editingId ?? '') || host.ephemeral) continue;
    if (host.label.toLowerCase() == lower) {
      return 'A host named "$name" already exists — pick a different name.';
    }
  }
  return null;
}
