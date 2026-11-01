/// The Hosts screen: a search+Connect row, the split [New host|▾] + Terminal +
/// Serial toolbar, a bold "主机" section title over the card grid, and cards
/// with monochrome OS glyph tiles plus a hover edit-pencil.
library;

import 'dart:async';

import 'package:flutter/foundation.dart' show ValueListenable, ValueNotifier;
import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../../bridge/native_bridge.dart';
import '../../data/host_os_registry.dart';
import '../../data/hosts.dart';
import '../../l10n/app_language.dart';
import '../../theme/app_field.dart';
import '../../theme/app_theme.dart';
import '../../widgets/primitives.dart';
import '../../widgets/toolbar.dart';
import 'host_card.dart';
import 'host_details_panel.dart';
import 'serial_details_panel.dart';

enum HostsLayout { cards, rows }

typedef HostSaver = Future<CoreWriteResult> Function({
  required String id,
  required String label,
  required String address,
  required int port,
  required String username,
  required String group,
  required String kind,
  String keyPath,
  String keyInline,
  bool clearKey,
  String? encoding,
  int? baudRate,
  int? dataBits,
  int? stopBits,
  String? parity,
  String? flowControl,
  String? password,
  bool? clearPassword,
  String? jumpHostId,
  List<Map<String, dynamic>>? triggers,
});
typedef HostDeleter = Future<CoreWriteResult> Function(String id);
typedef GroupLoader = Future<List<HostGroup>> Function();
typedef GroupCreator = Future<CoreWriteResult> Function(String name);
typedef GroupRenamer = Future<CoreWriteResult> Function(
  String old,
  String next,
);
typedef GroupDeleter = Future<CoreWriteResult> Function(String name);
typedef GroupCollapser = Future<CoreWriteResult> Function(
  String name,
  bool collapsed,
);
typedef HostImporter = Future<ImportResult> Function();

class HostsScreen extends StatefulWidget {
  final ValueChanged<Host>? onConnect;

  /// Session source; defaults to the Rust core through the FFI bridge.
  final Future<List<Host>> Function()? loadHosts;

  /// Incremented by the title bar's + button to open the New host drawer.
  final ValueListenable<int>? newHostRequest;

  /// Test seams; production uses the FFI bridge.
  final HostSaver? saveSession;
  final HostDeleter? deleteSession;

  /// Folder (group) source and mutations; production uses the FFI bridge.
  final GroupLoader? loadGroups;
  final GroupCreator? createGroup;
  final GroupRenamer? renameGroup;
  final GroupDeleter? deleteGroup;
  final GroupCollapser? collapseGroup;

  /// OpenSSH config importer; production reads `~/.ssh/config` through the core.
  final HostImporter? importHosts;

  /// Serial device enumeration for the serial modal (test seam).
  final SerialPortLoader? loadSerialPorts;

  const HostsScreen({
    super.key,
    this.onConnect,
    this.loadHosts,
    this.newHostRequest,
    this.saveSession,
    this.deleteSession,
    this.loadGroups,
    this.createGroup,
    this.renameGroup,
    this.deleteGroup,
    this.collapseGroup,
    this.importHosts,
    this.loadSerialPorts,
  });

  @override
  State<HostsScreen> createState() => _HostsScreenState();
}

class _HostsScreenState extends State<HostsScreen> {
  final TextEditingController _search = TextEditingController();
  HostsLayout _layout = HostsLayout.cards;
  Host? _drawerHost;
  bool _drawerOpen = false;
  String _query = '';

  /// Debounced filter query. Typing updates the field itself (no rebuild);
  /// only the results area listens here, at most once per
  /// [_searchDebounceTime] — a keystroke burst must not rebuild the header,
  /// toolbar and group tree every character.
  final ValueNotifier<String> _filterQuery = ValueNotifier<String>('');
  Timer? _searchDebounce;
  static const Duration _searchDebounceTime = Duration(milliseconds: 80);

  List<Host> _hosts = const [];

  /// Group names already in use — the drawers' Parent group picker options.
  List<String> get _knownGroups => {
    for (final host in _hosts)
      if (!host.ephemeral && host.group.trim().isNotEmpty) host.group.trim(),
  }.toList()
    ..sort();
  List<HostGroup> _groups = const [];
  bool _loading = true;
  String? _loadError;

  /// Inline group editor: null = closed; `renaming == null` = create.
  bool _groupEditorOpen = false;
  String? _renaming;
  final TextEditingController _groupName = TextEditingController();
  String? _groupError;

  /// Group awaiting delete confirmation.
  String? _deletingGroup;

  /// Serial modal: null host creates, non-null edits that session.
  bool _serialPanelOpen = false;
  Host? _serialHost;
  String? _serialError;

  /// The serial drawer was reached from the New host draft (type switch):
  /// submitting saves the session instead of opening a temporary tab.
  bool _serialSaveDraft = false;

  /// Result banner for the OpenSSH config import.
  String? _importNotice;
  bool _importError = false;
  bool _importing = false;

  @override
  void initState() {
    super.initState();
    widget.newHostRequest?.addListener(_onNewHostRequest);
    hostOsRegistry.addListener(_onOsDetected);
    _reload();
  }

  void _onOsDetected() {
    if (mounted) setState(() {});
  }

  @override
  void didUpdateWidget(HostsScreen oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.newHostRequest != widget.newHostRequest) {
      oldWidget.newHostRequest?.removeListener(_onNewHostRequest);
      widget.newHostRequest?.addListener(_onNewHostRequest);
    }
  }

  void _onNewHostRequest() {
    if (mounted) _createSession();
  }

  Future<void> _reload({bool silent = false}) async {
    if (!silent) {
      setState(() {
        _loading = true;
        _loadError = null;
      });
    }
    try {
      final hosts = await (widget.loadHosts ?? loadHostsFromCore)();
      // Widget tests inject only the host loader; don't reach for the FFI
      // group store unless production wiring (or a group seam) is present.
      final groups = widget.loadGroups != null
          ? await widget.loadGroups!()
          : widget.loadHosts != null
          ? const <HostGroup>[]
          : await loadGroupsFromCore();
      if (!mounted) return;
      setState(() {
        _hosts = hosts;
        _groups = groups;
        _loading = false;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _loading = false;
        _loadError = error is NativeBridgeError ? error.message : '$error';
      });
    }
  }

  @override
  void dispose() {
    widget.newHostRequest?.removeListener(_onNewHostRequest);
    hostOsRegistry.removeListener(_onOsDetected);
    _searchDebounce?.cancel();
    _filterQuery.dispose();
    _search.dispose();
    _groupName.dispose();
    super.dispose();
  }

  /// The search field's onChanged: keep the raw text for quick-connect and
  /// publish the debounced filter query that drives the results area.
  void _onSearchChanged(String value) {
    _query = value;
    _searchDebounce?.cancel();
    _searchDebounce = Timer(_searchDebounceTime, () {
      if (mounted) _filterQuery.value = _query;
    });
  }

  List<Host> _filteredFor(String query) {
    final q = query.trim().toLowerCase();
    if (q.isEmpty) return _hosts;
    return _hosts
        .where(
          (h) =>
              h.label.toLowerCase().contains(q) ||
              h.address.toLowerCase().contains(q) ||
              h.username.toLowerCase().contains(q),
        )
        .toList();
  }

  void _connect(Host? host) {
    if (host != null) widget.onConnect?.call(host);
  }

  /// Persists the drawer's edits into the core store, then refreshes the list.
  Future<String?> _saveHostToCore(
    Host host, {
    String password = '',
    bool clearPassword = false,
    String keyInline = '',
    bool clearKey = false,
  }) async {
    try {
      final result = await (widget.saveSession ?? saveSessionToCore)(
        id: host.id,
        label: host.label,
        address: host.address,
        port: host.port,
        username: host.username,
        group: host.group,
        kind: host.protocol,
        keyPath: host.keyPath,
        keyInline: keyInline,
        clearKey: clearKey,
        baudRate: host.baudRate,
        dataBits: host.dataBits,
        stopBits: host.stopBits,
        parity: host.parity,
        flowControl: host.flowControl,
        password: password,
        clearPassword: clearPassword,
        jumpHostId: host.jumpHostId,
        encoding: host.encoding,
        triggers: host.triggers.map((t) => t.toCoreJson()).toList(),
      );
      if (!result.ok) return result.error ?? 'Save failed';
      await _reload(silent: true);
      final id = result.id ?? host.id;
      for (final h in _hosts) {
        if (h.id == id) {
          if (mounted) setState(() => _drawerHost = h);
          break;
        }
      }
      return null;
    } catch (error) {
      return '$error';
    }
  }

  /// Deletes one session through the core and refreshes the list.
  Future<String?> _deleteHostToCore(Host host) async {
    try {
      final result = await (widget.deleteSession ?? deleteSessionFromCore)(
        host.id,
      );
      if (!result.ok) return result.error ?? 'Delete failed';
      _closeDrawer();
      await _reload(silent: true);
      return null;
    } catch (error) {
      return '$error';
    }
  }

  /// Creates a session in the core and returns it after the list reloads.
  Future<Host?> _createSessionFor({
    required String kind,
    required String label,
    String address = '',
    int port = 22,
    String username = '',
    String group = '',
    int baudRate = 115200,
    int dataBits = 8,
    int stopBits = 1,
    String parity = 'none',
    String flowControl = 'none',
    String password = '',
  }) async {
    try {
      final result = await (widget.saveSession ?? saveSessionToCore)(
        id: '',
        label: label,
        address: address,
        port: port,
        username: username,
        group: group,
        kind: kind,
        baudRate: baudRate,
        dataBits: dataBits,
        stopBits: stopBits,
        parity: parity,
        flowControl: flowControl,
        password: password,
      );
      if (!result.ok) {
        if (mounted) {
          setState(() => _loadError = result.error ?? 'Could not create host');
        }
        return null;
      }
      await _reload(silent: true);
      for (final host in _hosts) {
        if (host.id == result.id) return host;
      }
      return null;
    } catch (error) {
      if (mounted) setState(() => _loadError = '$error');
      return null;
    }
  }

  /// The New host button: open the drawer on an unsaved SSH draft. Nothing
  /// reaches the store until Save passes validation — no more empty
  /// "New Host" shells — and the drawer offers the connection-type switch.
  void _createSession() {
    setState(() {
      _serialHost = null;
      _serialPanelOpen = false;
      _drawerHost = Host(id: '', label: '', address: '', ephemeral: true);
      _drawerOpen = true;
    });
  }

  /// The draft drawer's connection-type switch: hand the new connection to
  /// the serial editor (same drawer slot). This path saves on submit.
  void _switchDraftToSerial() {
    setState(() {
      _serialHost = null;
      _serialError = null;
      _serialSaveDraft = true;
      _drawerHost = null;
      _serialPanelOpen = true;
      _drawerOpen = true;
    });
  }

  /// The serial draft's way back: reopen the SSH editor for a new host.
  void _switchDraftToSsh() {
    setState(() {
      _serialHost = null;
      _serialPanelOpen = false;
      _drawerHost = Host(id: '', label: '', address: '', ephemeral: true);
      _drawerOpen = true;
    });
  }

  /// The toolbar's Terminal action: a real local PTY session, opened directly.
  /// An existing "Local terminal" is reused instead of piling up duplicates.
  void _quickLocal() {
    // A local shell is a temporary tab: the toolbar is its dedicated entry, so
    // there is nothing to persist and nothing to clutter Home with.
    _connect(
      Host(
        id: 'adhoc-local-${DateTime.now().microsecondsSinceEpoch}',
        label: AppLanguage.tr(context, 'Local terminal'),
        address: '',
        username: '',
        protocol: 'local',
        ephemeral: true,
      ),
    );
  }

  /// The toolbar's Serial action: opens the serial connection drawer, where the
  /// port and line parameters are chosen before the session exists. This path
  /// is a temporary tab (nothing is stored unless edited later).
  void _quickSerial() {
    setState(() {
      _serialHost = null;
      _serialError = null;
      _serialSaveDraft = false;
      _drawerHost = null;
      _serialPanelOpen = true;
      _drawerOpen = true;
    });
  }

  Future<void> _submitSerial(SerialDraft draft) async {
    // Same draft rules as the SSH editor: no empty connections, no duplicate
    // names. Checked here (not in the panel) because the screen owns _hosts.
    final label = draft.label.trim().isEmpty ? draft.port : draft.label.trim();
    final validation = validateHostDraft(
      label: label,
      address: draft.port,
      username: draft.username,
      kind: 'serial',
      hosts: _hosts,
      editingId: _serialHost?.id ?? '',
    );
    if (validation != null) {
      setState(() => _serialError = validation);
      return;
    }
    Host? target;
    final editing = _serialHost;
    if (editing == null && _serialSaveDraft) {
      // Reached through New host → Serial: persist like the SSH draft and
      // close the drawer instead of opening a throwaway tab.
      final created = Host(
        id: '',
        label: label,
        address: draft.port,
        port: 22,
        username: draft.username,
        group: draft.group,
        protocol: 'serial',
        baudRate: draft.baudRate,
        dataBits: draft.dataBits,
        stopBits: draft.stopBits,
        parity: draft.parity,
        flowControl: draft.flowControl,
      );
      final error = await _saveHostToCore(created, password: draft.password);
      if (error != null) {
        if (mounted) setState(() => _serialError = error);
        return;
      }
      if (!mounted) return;
      setState(() {
        _serialPanelOpen = false;
        _serialSaveDraft = false;
        _drawerOpen = false;
      });
      return;
    }
    if (editing == null) {
      // Connecting from the toolbar is temporary; only editing an existing
      // serial host writes to the store.
      target = Host(
        id: 'adhoc-serial-${DateTime.now().microsecondsSinceEpoch}',
        label: draft.label,
        address: draft.port,
        port: 22,
        username: draft.username,
        group: draft.group,
        protocol: 'serial',
        baudRate: draft.baudRate,
        dataBits: draft.dataBits,
        stopBits: draft.stopBits,
        parity: draft.parity,
        flowControl: draft.flowControl,
        ephemeral: true,
        adhocPassword: draft.password,
      );
    } else {
      final updated = Host(
        id: editing.id,
        label: draft.label,
        address: draft.port,
        port: editing.port,
        username: draft.username,
        group: draft.group,
        tags: editing.tags,
        os: editing.os,
        lastUsed: editing.lastUsed,
        protocol: 'serial',
        auth: editing.auth,
        keyPath: editing.keyPath,
        baudRate: draft.baudRate,
        dataBits: draft.dataBits,
        stopBits: draft.stopBits,
        parity: draft.parity,
        flowControl: draft.flowControl,
        hasPassword: editing.hasPassword || draft.password.isNotEmpty,
      );
      final error = await _saveHostToCore(updated, password: draft.password);
      if (error != null) {
        if (mounted) setState(() => _serialError = error);
        return;
      }
      for (final host in _hosts) {
        if (host.id == editing.id) {
          target = host;
          break;
        }
      }
      target ??= updated;
    }
    if (!mounted) return;
    setState(() {
      _serialPanelOpen = false;
      _drawerOpen = false;
    });
    _connect(target);
  }

  /// Quick connect from the search field: an existing host wins; otherwise a
  /// `user@host:port` query creates a new SSH session and connects immediately.
  Future<void> _quickConnect() async {
    final query = _query.trim();
    if (query.isEmpty) return;
    final lower = query.toLowerCase();
    for (final host in _hosts) {
      if (host.label.toLowerCase() == lower ||
          host.address.toLowerCase() == lower) {
        _connect(host);
        return;
      }
    }
    var user = '';
    var addressPort = query;
    final at = query.indexOf('@');
    if (at > 0) {
      user = query.substring(0, at);
      addressPort = query.substring(at + 1);
    }
    var address = addressPort;
    var port = 22;
    final colon = addressPort.lastIndexOf(':');
    if (colon > 0 && !addressPort.contains(']')) {
      address = addressPort.substring(0, colon);
      port = int.tryParse(addressPort.substring(colon + 1)) ?? 22;
    }
    if (address.isEmpty) return;
    final host = await _createSessionFor(
      kind: 'ssh',
      label: query,
      address: address,
      port: port,
      username: user,
    );
    if (host != null) _connect(host);
  }

  /// Collapse the slider; the panel content stays mounted for the next open.
  void _closeDrawer() => setState(() => _drawerOpen = false);

  void _closeSerialPanel() => setState(() {
    _serialPanelOpen = false;
    _serialSaveDraft = false;
    _drawerOpen = false;
  });

  // ── Groups (folders) ───────────────────────────────────────────────────

  void _openGroupEditor({String? rename}) {
    setState(() {
      _groupEditorOpen = true;
      _renaming = rename;
      _groupName.text = rename ?? '';
      _groupError = null;
      _deletingGroup = null;
    });
  }

  void _closeGroupEditor() => setState(() {
    _groupEditorOpen = false;
    _renaming = null;
    _groupError = null;
  });

  Future<void> _saveGroup() async {
    final name = _groupName.text.trim();
    if (name.isEmpty) {
      setState(() => _groupError = 'Enter a group name');
      return;
    }
    final renaming = _renaming;
    try {
      final result = renaming == null
          ? await (widget.createGroup ?? createGroupInCore)(name)
          : await (widget.renameGroup ?? renameGroupInCore)(renaming, name);
      if (!result.ok) {
        setState(
          () => _groupError = result.error ?? 'Could not save the group',
        );
        return;
      }
      if (!mounted) return;
      setState(() {
        _groupEditorOpen = false;
        _renaming = null;
        _groupError = null;
      });
      await _reload(silent: true);
    } catch (error) {
      if (!mounted) return;
      setState(() => _groupError = '$error');
    }
  }

  Future<void> _deleteGroupNamed(String name) async {
    try {
      final result = await (widget.deleteGroup ?? deleteGroupFromCore)(name);
      if (!result.ok) {
        setState(
          () => _loadError = result.error ?? 'Could not delete the group',
        );
        return;
      }
      if (!mounted) return;
      setState(() => _deletingGroup = null);
      await _reload(silent: true);
    } catch (error) {
      if (!mounted) return;
      setState(() => _loadError = '$error');
    }
  }

  Future<void> _toggleGroup(HostGroup group) async {
    final collapsed = !group.collapsed;
    setState(() {
      _groups = [
        for (final g in _groups)
          g.name == group.name
              ? HostGroup(name: g.name, collapsed: collapsed)
              : g,
      ];
    });
    try {
      final result = await (widget.collapseGroup ?? collapseGroupInCore)(
        group.name,
        collapsed,
      );
      if (!result.ok && mounted) {
        await _reload(silent: true);
      }
    } catch (_) {
      if (mounted) await _reload(silent: true);
    }
  }

  Future<void> _newHostInGroup(String group) async {
    final host = await _createSessionFor(
      kind: 'ssh',
      label: AppLanguage.tr(context, 'New Host'),
      group: group,
    );
    if (host == null || !mounted) return;
    setState(() {
      _drawerHost = host;
      _drawerOpen = true;
    });
  }

  /// Imports `~/.ssh/config` through the core; the result stays in a banner.
  Future<void> _importHosts() async {
    setState(() {
      _importing = true;
      _importNotice = null;
      _importError = false;
    });
    try {
      final result = await (widget.importHosts ?? importSshConfigToCore)();
      if (!mounted) return;
      final added = result.added;
      final imported = added == 0
          ? AppLanguage.tr(context, 'No new hosts to import')
          : added == 1
          ? AppLanguage.tr(context, 'Imported 1 host')
          : AppLanguage.tr(context, 'Imported {count} hosts', {
              'count': added,
            });
      setState(() {
        _importing = false;
        _importError = !result.ok;
        _importNotice = result.ok
            ? imported
            : (result.error ?? 'Import failed');
      });
      if (result.ok && result.added > 0) await _reload(silent: true);
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _importing = false;
        _importError = true;
        _importNotice = '$error';
      });
    }
  }

  /// A plain card click connects immediately — the desktop fast path.
  void _onCardTap(Host host) => _connect(host);

  /// The hover pencil opens the matching editor: serial hosts use the serial
  /// modal (line parameters), everything else the SSH details drawer.
  void _onEditHost(Host host) {
    if (host.isSerial) {
      setState(() {
        _serialHost = host;
        _serialError = null;
        _serialSaveDraft = false;
        _drawerHost = null;
        _serialPanelOpen = true;
        _drawerOpen = true;
      });
      return;
    }
    setState(() {
      _serialPanelOpen = false;
      _drawerHost = host;
      _drawerOpen = true;
    });
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    final hostsColumn = Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // The hosts content: search row, toolbar, section title, grid. The
        // drawer overlays the right edge (see the Stack below) so the grid
        // never reflows while the panel animates.
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              // ── Header band: search row + toolbar ──────────────────────
              // The hosts header sits on the raised surface (the navigation
              // rail shares it) instead of on the page background.
              Container(
                color: XtColors.surface(context),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    // Search + Connect (inside the field).
                    Padding(
                      padding: const EdgeInsets.fromLTRB(18, 8, 18, 0),
                      child: XtField(
                        key: const ValueKey('host-search'),
                        controller: _search,
                        placeholder: AppLanguage.tr(
                          context,
                          'Find a host or ssh user@hostname...',
                        ),
                        onChanged: _onSearchChanged,
                        // Connect lives INSIDE the field and stays quiet; the
                        // drawer's Connect is the screen's primary action.
                        trailing: ValueListenableBuilder<String>(
                          valueListenable: _filterQuery,
                          builder: (context, query, _) => XtActionButton(
                            key: const ValueKey('quick-connect'),
                            label: AppLanguage.tr(context, 'Connect'),
                            height: 28,
                            onPressed: query.trim().isEmpty
                                ? null
                                : _quickConnect,
                          ),
                        ),
                      ),
                    ),
                    // Toolbar row.
                    Padding(
                      padding: const EdgeInsets.fromLTRB(18, 10, 18, 10),
                      child: Row(
                        children: [
                          SplitButton(
                            label: AppLanguage.tr(context, 'New host'),
                            icon: LucideIcons.plus,
                            onPressed: _createSession,
                            menuItems: [
                              SplitMenuEntry(
                                icon: LucideIcons.folderPlus,
                                label: AppLanguage.tr(context, 'New group'),
                                onTap: _openGroupEditor,
                              ),
                              SplitMenuEntry(
                                icon: LucideIcons.import,
                                label: AppLanguage.tr(context, 'Import hosts'),
                                onTap: _importHosts,
                              ),
                            ],
                          ),
                          const SizedBox(width: 20),
                          GhostAction(
                            icon: LucideIcons.terminal,
                            label: AppLanguage.tr(context, 'Terminal'),
                            onPressed: _quickLocal,
                          ),
                          const SizedBox(width: 20),
                          GhostAction(
                            icon: LucideIcons.ethernetPort,
                            label: AppLanguage.tr(context, 'Serial'),
                            onPressed: _quickSerial,
                          ),
                          const Spacer(),
                          IconToggleButton(
                            icon: _layout == HostsLayout.cards
                                ? LucideIcons.layoutGrid
                                : LucideIcons.list,
                            onPressed: () => setState(
                              () => _layout = _layout == HostsLayout.cards
                                  ? HostsLayout.rows
                                  : HostsLayout.cards,
                            ),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
              // ── Inline group editor / import result ─────────────────────
              if (_groupEditorOpen) _groupEditorBar(theme),
              if (_importNotice != null) _importBanner(theme),
              // ── Section titles + cards ──────────────────────────────────
              // Only this subtree listens to the debounced search query: a
              // keystroke never rebuilds the header band or the drawer.
              Expanded(
                child: ValueListenableBuilder<String>(
                  valueListenable: _filterQuery,
                  builder: (context, query, _) {
                    final hosts = _filteredFor(query);
                    return Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        if (_groups.isEmpty || query.trim().isNotEmpty)
                          _sectionTitle('Home'),
                        Expanded(
                          child: RepaintBoundary(
                            child: Padding(
                              padding: const EdgeInsets.fromLTRB(18, 0, 18, 16),
                              child: _loading
                                  ? const Center(
                                      child: CircularProgressIndicator(
                                        size: 20,
                                        strokeWidth: 2,
                                      ),
                                    )
                                  : _loadError != null
                                  ? XtEmptyState(
                                      icon: LucideIcons.circleAlert,
                                      title: AppLanguage.tr(
                                        context,
                                        'Could not load hosts',
                                      ),
                                      message: AppLanguage.tr(
                                        context,
                                        _loadError!,
                                      ),
                                      danger: true,
                                      action: XtActionButton(
                                        label: AppLanguage.tr(context, 'Retry'),
                                        onPressed: _reload,
                                      ),
                                    )
                                  : hosts.isEmpty && _groups.isEmpty
                                  ? _emptyState
                                  : _groups.isEmpty ||
                                        query.trim().isNotEmpty
                                  ? (hosts.isEmpty
                                        ? _emptyState
                                        : _layout == HostsLayout.cards
                                        ? _cardGrid(hosts)
                                        : _rowList(hosts))
                                  : _groupedList(theme),
                            ),
                          ),
                        ),
                      ],
                    );
                  },
                ),
              ),
            ],
          ),
        ),
      ],
    );
    // The details drawer overlays the right edge instead of reflowing the
    // hosts column: a 340px macOS-inspector-style panel slid in by pure
    // transform, so neither the grid nor the header relayouts per frame.
    final drawer = _DrawerSlider(
          open: _drawerOpen,
          child: _serialPanelOpen
              ? SerialDetailsPanel(
                  host: _serialHost,
                  loadPorts: widget.loadSerialPorts,
                  error: _serialError,
                  saveMode: _serialSaveDraft,
                  groups: _knownGroups,
                  onSwitchToSsh:
                      _serialHost == null ? _switchDraftToSsh : null,
                  onSubmit: _submitSerial,
                  onClose: _closeSerialPanel,
                  // A saved serial session deletes through the same core path
                  // as SSH/Telnet/local; a draft has no stored id.
                  onDelete: _serialHost != null && _serialHost!.id.isNotEmpty
                      ? _deleteHostToCore
                      : null,
                )
              : _drawerHost == null
              ? const SizedBox.shrink()
              : HostDetailsPanel(
                  host: _drawerHost!,
                  hosts: _hosts,
                  onClose: _closeDrawer,
                  onSwitchToSerial:
                      _drawerHost!.id.isEmpty ? _switchDraftToSerial : null,
                  onKindChanged: _drawerHost!.id.isEmpty
                      ? (kind) => setState(() {
                          final draft = _drawerHost!;
                          _drawerHost = Host(
                            id: draft.id,
                            label: draft.label,
                            address: draft.address,
                            port: draft.port,
                            username: draft.username,
                            group: draft.group,
                            protocol: kind,
                            ephemeral: true,
                          );
                        })
                      : null,
                  onConnect: () {
                    final host = _drawerHost;
                    _closeDrawer();
                    _connect(host);
                  },
                  onSave: (
                    host,
                    password,
                    clearPassword,
                    keyInline,
                    clearKey,
                  ) => _saveHostToCore(
                    host,
                    password: password,
                    clearPassword: clearPassword,
                    keyInline: keyInline,
                    clearKey: clearKey,
                  ),
                  onDelete: _deleteHostToCore,
                ),
    );
    return Stack(
      children: [
        Positioned.fill(child: hostsColumn),
        Positioned(top: 0, bottom: 0, right: 0, child: drawer),
      ],
    );
  }

  Widget _groupEditorBar(ThemeData theme) {
    final renaming = _renaming;
    return Container(
      color: XtColors.surface(context),
      padding: const EdgeInsets.fromLTRB(18, 10, 18, 10),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Icon(
                renaming == null ? LucideIcons.folderPlus : LucideIcons.pencil,
                size: 16,
                color: XtColors.textSecondary(context),
              ),
              const SizedBox(width: 8),
              Text(
                AppLanguage.tr(
                  context,
                  renaming == null ? 'New group' : 'Rename group',
                ),
                style: theme.typography.medium.copyWith(
                  fontSize: 14,
                  color: XtColors.textPrimary(context),
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(
                child: XtField(
                  key: const ValueKey('group-name'),
                  controller: _groupName,
                  autofocus: true,
                  onSubmitted: (_) => _saveGroup(),
                  placeholder: AppLanguage.tr(
                    context,
                    'Group name (for example: Production)',
                  ),
                ),
              ),
              const SizedBox(width: 10),
              XtActionButton(
                key: const ValueKey('group-save'),
                label: AppLanguage.tr(context, 'Save'),
                variant: XtActionVariant.primary,
                onPressed: _saveGroup,
              ),
              const SizedBox(width: 8),
              XtActionButton(
                key: const ValueKey('group-cancel'),
                label: AppLanguage.tr(context, 'Cancel'),
                onPressed: _closeGroupEditor,
              ),
            ],
          ),
          if (_groupError != null) ...[
            const SizedBox(height: 6),
            Text(
              AppLanguage.tr(context, _groupError!),
              style: theme.typography.normal.copyWith(
                fontSize: 12,
                color: XtColors.dangerText(context),
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _importBanner(ThemeData theme) {
    return Container(
      key: const ValueKey('import-banner'),
      color: _importError
          ? XtColors.dangerTint(context)
          : XtColors.sunken(context),
      padding: const EdgeInsets.fromLTRB(18, 8, 10, 8),
      child: Row(
        children: [
          Icon(
            _importing
                ? LucideIcons.loader
                : _importError
                ? LucideIcons.circleAlert
                : LucideIcons.circleCheck,
            size: 15,
            color: _importError
                ? XtColors.dangerText(context)
                : XtColors.textPrimary(context),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              AppLanguage.tr(context, _importNotice!),
              style: theme.typography.normal.copyWith(
                fontSize: 12,
                color: XtColors.textPrimary(context),
              ),
            ),
          ),
          XtIconButton(
            key: const ValueKey('import-dismiss'),
            icon: LucideIcons.x,
            size: 24,
            iconSize: 14,
            onPressed: () => setState(() => _importNotice = null),
          ),
        ],
      ),
    );
  }

  Widget _sectionTitle(String title) {
    return XtSectionTitle(
      AppLanguage.tr(context, title),
      padding: const EdgeInsets.fromLTRB(18, 15, 0, 12),
    );
  }

  /// Named groups plus the ungrouped bucket, in core display order.
  List<_HostSection> get _sections {
    final byGroup = <String, List<Host>>{};
    final ungrouped = <Host>[];
    for (final host in _hosts) {
      final group = host.group.trim();
      final reserved =
          group.toLowerCase() == 'default' || group.toLowerCase() == 'system';
      if (group.isEmpty || reserved) {
        ungrouped.add(host);
      } else {
        byGroup.putIfAbsent(group, () => []).add(host);
      }
    }
    final known = <String>{
      for (final group in _groups) group.name.toLowerCase(),
    };
    final names = <String>[
      for (final group in _groups) group.name,
      for (final name in byGroup.keys)
        if (!known.contains(name.toLowerCase())) name,
    ]..sort((a, b) => a.toLowerCase().compareTo(b.toLowerCase()));
    return [
      if (ungrouped.isNotEmpty || names.isEmpty)
        _HostSection(name: null, collapsed: false, hosts: ungrouped),
      for (final name in names)
        _HostSection(
          name: name,
          collapsed: _groups.any((g) => g.name == name && g.collapsed),
          hosts: byGroup[name] ?? const [],
        ),
    ];
  }

  Widget _groupedList(ThemeData theme) {
    final sections = _sections;
    return ListView(
      padding: EdgeInsets.zero,
      children: [
        for (final section in sections) ...[
          if (section.name == null)
            _sectionTitle('Home')
          else
            _deletingGroup == section.name
                ? _deleteGroupBar(section.name!, theme)
                : _GroupHeader(
                    key: ValueKey('group-header-${section.name}'),
                    name: section.name!,
                    count: section.hosts.length,
                    collapsed: section.collapsed,
                    theme: theme,
                    onToggle: () => _toggleGroup(
                      HostGroup(
                        name: section.name!,
                        collapsed: section.collapsed,
                      ),
                    ),
                    onRename: () => _openGroupEditor(rename: section.name),
                    onDelete: () =>
                        setState(() => _deletingGroup = section.name),
                    onAdd: () => _newHostInGroup(section.name!),
                  ),
          if (section.name == null || !section.collapsed)
            Padding(
              padding: EdgeInsets.fromLTRB(
                section.name == null ? 0 : 12,
                0,
                0,
                section.name == null ? 0 : 14,
              ),
              child: section.hosts.isEmpty
                  ? Padding(
                      padding: const EdgeInsets.symmetric(vertical: 8),
                      child: Text(
                        AppLanguage.tr(
                          context,
                          'No hosts in this group yet.',
                        ),
                        style: theme.typography.normal.copyWith(
                          fontSize: 12,
                          color: XtColors.textTertiary(context),
                        ),
                      ),
                    )
                  : _layout == HostsLayout.cards
                  ? _cardGrid(section.hosts, shrinkWrap: true)
                  : _rowList(section.hosts, shrinkWrap: true),
            ),
        ],
      ],
    );
  }

  Widget _deleteGroupBar(String name, ThemeData theme) {
    return Container(
      margin: const EdgeInsets.only(bottom: 10),
      padding: const EdgeInsets.fromLTRB(14, 10, 10, 10),
      decoration: BoxDecoration(
        color: XtColors.surface(context),
        borderRadius: BorderRadius.circular(XtRadius.field),
        border: Border.all(color: XtColors.hairline(context)),
      ),
      child: Row(
        children: [
          Icon(
            LucideIcons.triangleAlert,
            size: 16,
            color: XtColors.danger(context),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              AppLanguage.tr(
                context,
                'Delete “{name}”? Its hosts become ungrouped.',
                {'name': name},
              ),
              style: theme.typography.normal.copyWith(
                fontSize: 13,
                color: XtColors.textPrimary(context),
              ),
            ),
          ),
          XtActionButton(
            key: const ValueKey('group-delete-confirm'),
            label: AppLanguage.tr(context, 'Delete'),
            variant: XtActionVariant.danger,
            onPressed: () => _deleteGroupNamed(name),
          ),
          const SizedBox(width: 8),
          XtActionButton(
            label: AppLanguage.tr(context, 'Cancel'),
            onPressed: () => setState(() => _deletingGroup = null),
          ),
        ],
      ),
    );
  }

  /// Applies the OS detected on the last connection without touching the
  /// stored session.
  Host _displayed(Host host) => host.copyWith(os: hostOsRegistry.of(host.id));

  /// The empty Home placeholder: the core returned no hosts to show.
  Widget get _emptyState => XtEmptyState(
    icon: LucideIcons.server,
    title: AppLanguage.tr(context, 'No hosts yet'),
    message: AppLanguage.tr(
      context,
      'Hosts saved by the XenTerm core appear here.',
    ),
  );

  Widget _cardGrid(List<Host> hosts, {bool shrinkWrap = false}) {
    return LayoutBuilder(
      builder: (context, constraints) {
        const gap = 10.0;
        // Keep cards wide enough to read when the window shrinks; a fixed
        // column count squeezes them down to an unusable width instead.
        const minCardWidth = 260.0;
        final available = constraints.maxWidth;
        final columns = available.isFinite
            ? ((available + gap) / (minCardWidth + gap)).floor().clamp(1, 6)
            : 3;
        return GridView.builder(
          shrinkWrap: shrinkWrap,
          physics: shrinkWrap ? const NeverScrollableScrollPhysics() : null,
          padding: EdgeInsets.zero,
          gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
            crossAxisCount: columns,
            mainAxisSpacing: gap,
            crossAxisSpacing: gap,
            mainAxisExtent: 60,
          ),
          itemCount: hosts.length,
          itemBuilder: (context, index) {
            final host = _displayed(hosts[index]);
            return HostCard.grid(
              host: host,
              selected: _drawerOpen && _drawerHost?.id == host.id,
              onTap: () => _onCardTap(hosts[index]),
              onEdit: () => _onEditHost(hosts[index]),
            );
          },
        );
      },
    );
  }

  Widget _rowList(List<Host> hosts, {bool shrinkWrap = false}) {
    return ListView.builder(
      shrinkWrap: shrinkWrap,
      physics: shrinkWrap ? const NeverScrollableScrollPhysics() : null,
      padding: EdgeInsets.zero,
      itemCount: hosts.length,
      itemBuilder: (context, index) {
        final host = _displayed(hosts[index]);
        return Padding(
          padding: const EdgeInsets.only(bottom: 8),
          child: HostCard.row(
            host: host,
            selected: _drawerOpen && _drawerHost?.id == host.id,
            onTap: () => _onCardTap(hosts[index]),
            onEdit: () => _onEditHost(hosts[index]),
          ),
        );
      },
    );
  }
}

class _HostSection {
  final String? name;
  final bool collapsed;
  final List<Host> hosts;

  const _HostSection({
    required this.name,
    required this.collapsed,
    required this.hosts,
  });
}

/// Folder header: chevron + folder glyph + name + count, with hover actions.
class _GroupHeader extends StatefulWidget {
  final String name;
  final int count;
  final bool collapsed;
  final ThemeData theme;
  final VoidCallback onToggle;
  final VoidCallback onRename;
  final VoidCallback onDelete;
  final VoidCallback onAdd;

  const _GroupHeader({
    super.key,
    required this.name,
    required this.count,
    required this.collapsed,
    required this.theme,
    required this.onToggle,
    required this.onRename,
    required this.onDelete,
    required this.onAdd,
  });

  @override
  State<_GroupHeader> createState() => _GroupHeaderState();
}

class _GroupHeaderState extends State<_GroupHeader> {
  bool _hovering = false;

  @override
  Widget build(BuildContext context) {
    final theme = widget.theme;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hovering = true),
      onExit: (_) => setState(() => _hovering = false),
      child: XtHoverRow(
        height: 38,
        margin: const EdgeInsets.only(bottom: 8),
        padding: const EdgeInsets.only(left: 4, right: 4),
        onTap: widget.onToggle,
        child: Row(
          children: [
            AnimatedRotation(
              turns: widget.collapsed ? -0.25 : 0,
              duration: XtMotion.micro,
              curve: XtMotion.easeOut,
              child: Icon(
                LucideIcons.chevronDown,
                size: 16,
                color: XtColors.textSecondary(context),
              ),
            ),
            const SizedBox(width: 6),
            Icon(
              widget.collapsed ? LucideIcons.folder : LucideIcons.folderOpen,
              size: 16,
              color: XtColors.textSecondary(context),
            ),
            const SizedBox(width: 8),
            Text(
              widget.name,
              style: theme.typography.semiBold.copyWith(
                fontSize: 15,
                color: XtColors.textPrimary(context),
              ),
            ),
            const SizedBox(width: 8),
            Text(
              '${widget.count}',
              style: theme.typography.normal.copyWith(
                fontSize: 12,
                color: XtColors.textTertiary(context),
              ),
            ),
            const Spacer(),
            AnimatedOpacity(
              opacity: _hovering ? 1 : 0,
              duration: XtMotion.micro,
              child: Row(
                children: [
                  XtIconButton(
                    key: ValueKey('group-add-${widget.name}'),
                    icon: LucideIcons.plus,
                    tooltip: AppLanguage.tr(
                      context,
                      'New host in {group}',
                      {'group': widget.name},
                    ),
                    onPressed: widget.onAdd,
                    size: 28,
                  ),
                  XtIconButton(
                    key: ValueKey('group-rename-${widget.name}'),
                    icon: LucideIcons.pencil,
                    tooltip: AppLanguage.tr(context, 'Rename group'),
                    onPressed: widget.onRename,
                    size: 28,
                  ),
                  XtIconButton(
                    key: ValueKey('group-delete-${widget.name}'),
                    icon: LucideIcons.trash2,
                    tooltip: AppLanguage.tr(context, 'Delete group'),
                    onPressed: widget.onDelete,
                    size: 28,
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// The host-details / serial drawer: a fixed 340px panel slid in from the
/// right by a composited transform (200ms, ease-out).
///
/// The panel is laid out once at its real size and only its paint offset
/// changes per frame. Unlike the previous width-based [Align] animation this
/// never relayouts the host grid behind it — the drawer behaves like a macOS
/// inspector that the window slides over, and FrameTiming shows the build cost
/// drop to the cost of an empty animation turned callbacks.
class _DrawerSlider extends StatefulWidget {
  static const double width = 340;
  final bool open;
  final Widget child;

  const _DrawerSlider({required this.open, required this.child});

  @override
  State<_DrawerSlider> createState() => _DrawerSliderState();
}

class _DrawerSliderState extends State<_DrawerSlider>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller = AnimationController(
    vsync: this,
    duration: XtMotion.micro,
    reverseDuration: XtMotion.micro,
    value: widget.open ? 1 : 0,
  );
  late final Animation<double> _slide = CurvedAnimation(
    parent: _controller,
    curve: XtMotion.easeOut,
  );

  @override
  void didUpdateWidget(_DrawerSlider oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.open != oldWidget.open) {
      widget.open ? _controller.forward() : _controller.reverse();
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    // A hairline on the panel's leading edge reads as 'inspector' and costs one
    // line of paint.
    final panel = DecoratedBox(
      decoration: BoxDecoration(
        border: Border(left: BorderSide(color: XtColors.hairline(context))),
      ),
      child: widget.child,
    );
    return AnimatedBuilder(
      animation: _slide,
      builder: (context, child) {
        final value = _slide.value;
        // Fully closed: keep the (still attached) panel out of the hit test so
        // it can never eat a click meant for the grid under its edge.
        return IgnorePointer(
          ignoring: value == 0,
          child: ClipRect(
            child: Transform.translate(
              offset: Offset((1 - value) * _DrawerSlider.width, 0),
              child: child,
            ),
          ),
        );
      },
      child: RepaintBoundary(
        child: SizedBox(width: _DrawerSlider.width, child: panel),
      ),
    );
  }
}
