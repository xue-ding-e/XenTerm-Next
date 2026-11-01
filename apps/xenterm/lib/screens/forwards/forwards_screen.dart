/// Port Forwarding — the saved forwarding rules from every SSH session, with
/// real create/edit/delete through the core store. Rules are applied when the
/// owning session connects (the core's tunnel worker), so the list has no fake
/// global pause switch.
library;

import 'dart:async';

import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../../bridge/native_bridge.dart';
import '../../data/hosts.dart';
import '../../shell/window_activity.dart';
import '../../l10n/app_language.dart';
import '../../theme/app_field.dart';
import '../../theme/app_theme.dart';
import '../../widgets/primitives.dart';
import '../../widgets/toolbar.dart';

typedef ForwardLoader = Future<List<ForwardRule>> Function();
typedef ForwardSaver = Future<CoreWriteResult> Function({
  required String sessionId,
  required int index,
  required String kind,
  required String name,
  required String bindAddr,
  required String bindPort,
  required String host,
  required String hostPort,
});
typedef ForwardDeleter = Future<CoreWriteResult> Function({
  required String sessionId,
  required int index,
});
typedef ForwardHostLoader = Future<List<Host>> Function();
typedef JumpChainLoader = Future<List<JumpChain>> Function();

class ForwardsScreen extends StatefulWidget {
  final ForwardLoader? loadForwards;
  final ForwardSaver? saveForward;
  final ForwardDeleter? deleteForward;
  final ForwardHostLoader? loadHosts;
  final JumpChainLoader? loadJumps;

  /// Tunnel-state source; defaults to the running sessions through the bridge.
  final Future<List<RuntimeTunnel>> Function()? loadTunnels;

  const ForwardsScreen({
    super.key,
    this.loadForwards,
    this.saveForward,
    this.deleteForward,
    this.loadHosts,
    this.loadJumps,
    this.loadTunnels,
  });

  @override
  State<ForwardsScreen> createState() => _ForwardsScreenState();
}

class _ForwardsScreenState extends State<ForwardsScreen> {
  List<ForwardRule> _rules = const [];
  List<JumpChain> _jumps = const [];
  List<Host> _sshHosts = const [];
  List<RuntimeTunnel> _tunnels = const [];
  Timer? _tunnelPoll;
  bool _loading = true;
  String? _error;
  String? _notice;
  bool _searching = false;
  String _query = '';
  bool _drawerOpen = false;
  ForwardRule? _editing;
  final TextEditingController _search = TextEditingController();

  @override
  void initState() {
    super.initState();
    _reload();
    windowActivity.idle.addListener(_onWindowActivity);
    // Tunnel state lives only in open sessions; a light poll keeps the
    // "Active" section honest while this screen is on screen. Polling pauses
    // while the window itself is animating (resize/zoom/minimize) so it never
    // competes with the compositor for the frame budget.
    _startTunnelPoll();
  }

  void _onWindowActivity() {
    if (windowActivity.isIdle) {
      _startTunnelPoll();
      _pollTunnels();
    } else {
      _tunnelPoll?.cancel();
      _tunnelPoll = null;
    }
  }

  void _startTunnelPoll() {
    _tunnelPoll?.cancel();
    if (!windowActivity.isIdle) return;
    _tunnelPoll = Timer.periodic(
      const Duration(seconds: 2),
      (_) => _pollTunnels(),
    );
  }

  Future<void> _pollTunnels() async {
    try {
      final tunnels =
          await (widget.loadTunnels ?? loadRuntimeTunnelsFromCore)();
      if (!mounted) return;
      setState(() => _tunnels = tunnels);
    } catch (_) {
      // A dead bridge must not spam the screen; the next tick retries.
    }
  }

  @override
  void dispose() {
    windowActivity.idle.removeListener(_onWindowActivity);
    _tunnelPoll?.cancel();
    _search.dispose();
    super.dispose();
  }

  Future<void> _reload({bool silent = false}) async {
    if (!silent) {
      setState(() {
        _loading = true;
        _error = null;
      });
    }
    try {
      final rules = await (widget.loadForwards ?? loadForwardsFromCore)();
      final hosts = await (widget.loadHosts ?? loadHostsFromCore)();
      final jumps = widget.loadJumps != null
          ? await widget.loadJumps!()
          : widget.loadHosts != null
          ? const <JumpChain>[]
          : await loadJumpChainsFromCore();
      if (!mounted) return;
      setState(() {
        _rules = rules;
        _jumps = jumps;
        _sshHosts = hosts.where((h) => h.protocol == 'ssh').toList();
        _loading = false;
        _error = null;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = error is NativeBridgeError ? error.message : '$error';
      });
    }
  }

  Future<String?> _save({
    required String sessionId,
    required int index,
    required String kind,
    required String name,
    required String bindAddr,
    required String bindPort,
    required String host,
    required String hostPort,
  }) async {
    try {
      final result = await (widget.saveForward ?? saveForwardToCore)(
        sessionId: sessionId,
        index: index,
        kind: kind,
        name: name,
        bindAddr: bindAddr,
        bindPort: bindPort,
        host: host,
        hostPort: hostPort,
      );
      if (!result.ok) return result.error ?? 'Save failed';
      await _reload(silent: true);
      return null;
    } catch (error) {
      return '$error';
    }
  }

  Future<String?> _delete(ForwardRule rule) async {
    try {
      final result = await (widget.deleteForward ?? deleteForwardFromCore)(
        sessionId: rule.sessionId,
        index: rule.index,
      );
      if (!result.ok) return result.error ?? 'Delete failed';
      _closeDrawer();
      await _reload(silent: true);
      return null;
    } catch (error) {
      return '$error';
    }
  }

  void _openNew() {
    if (_sshHosts.isEmpty) {
      setState(() => _notice = 'Add an SSH host before creating a rule.');
      return;
    }
    setState(() => _notice = null);
    setState(() {
      _editing = null;
      _drawerOpen = true;
    });
  }

  void _openEdit(ForwardRule rule) {
    setState(() {
      _editing = rule;
      _drawerOpen = true;
    });
  }

  void _closeDrawer() {
    setState(() {
      _drawerOpen = false;
      _editing = null;
    });
  }

  List<ForwardRule> get _filtered {
    final query = _query.trim().toLowerCase();
    if (query.isEmpty) return _rules;
    return _rules
        .where(
          (r) =>
              r.name.toLowerCase().contains(query) ||
              r.sessionName.toLowerCase().contains(query) ||
              r.bind.toLowerCase().contains(query) ||
              r.target.toLowerCase().contains(query),
        )
        .toList();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
                child: Row(
                  children: [
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            AppLanguage.tr(context, 'Tunnels & Jumps'),
                            style: theme.typography.bold.copyWith(
                              fontSize: 18,
                              color: XtColors.textPrimary(context),
                            ),
                          ),
                          const SizedBox(height: 2),
                          Text(
                            AppLanguage.tr(
                              context,
                              'Port forwards and the jump hosts their sessions use.',
                            ),
                            style: theme.typography.normal.copyWith(
                              fontSize: 12,
                              color: XtColors.textSecondary(context),
                            ),
                          ),
                        ],
                      ),
                    ),
                    XtActionButton(
                      key: const ValueKey('forward-new'),
                      icon: LucideIcons.plus,
                      label: AppLanguage.tr(context, 'New rule'),
                      variant: XtActionVariant.primary,
                      onPressed: _openNew,
                    ),
                    const SizedBox(width: 6),
                    IconToggleButton(
                      icon: LucideIcons.search,
                      onPressed: () => setState(() {
                        _searching = !_searching;
                        if (!_searching) {
                          _search.clear();
                          _query = '';
                        }
                      }),
                    ),
                    const SizedBox(width: 6),
                    IconToggleButton(
                      icon: LucideIcons.refreshCw,
                      onPressed: () => _reload(),
                    ),
                  ],
                ),
              ),
              if (_searching)
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 8, 16, 4),
                  child: XtField(
                    key: const ValueKey('forward-search'),
                    controller: _search,
                    autofocus: true,
                    onChanged: (value) => setState(() => _query = value),
                    placeholder: AppLanguage.tr(context, 'Search rules'),
                  ),
                ),
              if (_notice != null)
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
                  child: Container(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 10,
                      vertical: 8,
                    ),
                    decoration: BoxDecoration(
                      color: XtColors.selected(context),
                      borderRadius: BorderRadius.circular(XtRadius.field),
                    ),
                    child: Text(
                      AppLanguage.tr(context, _notice!),
                      style: TextStyle(
                        fontSize: 12,
                        color: XtColors.textPrimary(context),
                      ),
                    ),
                  ),
                ),
              Expanded(
                child: SingleChildScrollView(
                  padding: const EdgeInsets.fromLTRB(16, 8, 16, 16),
                  // Center loosens the scroll view's tight width so the
                  // ConstrainedBox cap actually applies; without it the cards
                  // stretch to the window edge.
                  child: Center(
                    child: ConstrainedBox(
                      constraints: const BoxConstraints(maxWidth: 860),
                      child: _body(theme),
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
        ClipRect(
          child: AnimatedContainer(
            duration: XtMotion.micro,
            curve: XtMotion.easeOut,
            width: _drawerOpen ? 340 : 0,
            child: _drawerOpen
                ? OverflowBox(
                    alignment: Alignment.centerRight,
                    minWidth: 340,
                    maxWidth: 340,
                    child: ForwardEditor(
                      key: ValueKey(
                        '${_editing?.sessionId}-${_editing?.index}',
                      ),
                      rule: _editing,
                      hosts: _sshHosts,
                      onSave: _save,
                      onDelete: _editing == null ? null : _delete,
                      onClose: _closeDrawer,
                    ),
                  )
                : const SizedBox.shrink(),
          ),
        ),
      ],
    );
  }

  Widget _body(ThemeData theme) {
    if (_loading) {
      return Padding(
        padding: const EdgeInsets.all(32),
        child: Center(
          child: Text(
            AppLanguage.tr(context, 'Loading port forwards…'),
            style: TextStyle(color: XtColors.textTertiary(context)),
          ),
        ),
      );
    }
    if (_error != null) {
      return Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          children: [
            Text(
              AppLanguage.tr(context, _error!),
              textAlign: TextAlign.center,
              style: TextStyle(color: XtColors.danger(context)),
            ),
            const SizedBox(height: 12),
            XtActionButton(
              label: AppLanguage.tr(context, 'Retry'),
              variant: XtActionVariant.secondary,
              onPressed: () => _reload(),
            ),
          ],
        ),
      );
    }
    final rules = _filtered;
    final showJumps = _query.trim().isEmpty;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (_tunnels.isNotEmpty) ...[
          Row(
            children: [
              Text(
                AppLanguage.tr(context, 'Active tunnels'),
                style: theme.typography.semiBold.copyWith(
                  fontSize: 15,
                  color: XtColors.textPrimary(context),
                ),
              ),
              const SizedBox(width: 8),
              Text(
                '${_tunnels.length}',
                style: TextStyle(
                  fontSize: 12,
                  color: XtColors.textTertiary(context),
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          for (final tunnel in _tunnels)
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: _TunnelCard(tunnel: tunnel),
            ),
          const SizedBox(height: 18),
        ],
        Row(
          children: [
            Text(
              AppLanguage.tr(context, 'Port forwarding'),
              style: theme.typography.semiBold.copyWith(
                fontSize: 15,
                color: XtColors.textPrimary(context),
              ),
            ),
            const SizedBox(width: 8),
            Text(
              '${_rules.length}',
              style: TextStyle(
                fontSize: 12,
                color: XtColors.textTertiary(context),
              ),
            ),
          ],
        ),
        const SizedBox(height: 8),
        if (rules.isEmpty)
          XtEmptyState(
            icon: LucideIcons.arrowLeftRight,
            title: AppLanguage.tr(
              context,
              _rules.isEmpty ? 'No forwarding rules' : 'No matches',
            ),
            message: AppLanguage.tr(
              context,
              _rules.isEmpty
                  ? 'Rules open their tunnels when the owning host connects.'
                  : 'Try a different search.',
            ),
          )
        else
          for (final rule in rules)
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: _RuleCard(
                rule: rule,
                onEdit: () => _openEdit(rule),
                onDelete: () => _delete(rule),
              ),
            ),
        if (showJumps) ...[
          const SizedBox(height: 18),
          Row(
            children: [
              Text(
                AppLanguage.tr(context, 'Jump chains'),
                style: theme.typography.semiBold.copyWith(
                  fontSize: 15,
                  color: XtColors.textPrimary(context),
                ),
              ),
              const SizedBox(width: 8),
              Text(
                '${_jumps.length}',
                style: TextStyle(
                  fontSize: 12,
                  color: XtColors.textTertiary(context),
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          if (_jumps.isEmpty)
            XtEmptyState(
              icon: LucideIcons.route,
              title: AppLanguage.tr(context, 'No jump hosts configured'),
              message: AppLanguage.tr(
                context,
                'Pick a jump host under a host\'s SSH details to route it '
                'through another server.',
              ),
            )
          else
            for (final jump in _jumps)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: _JumpCard(chain: jump),
              ),
        ],
      ],
    );
  }

  static Color kindColor(BuildContext context, String kind) => switch (kind) {
    'local' => XtColors.accent(context),
    'remote' => XtColors.accent(context),
    _ => XtColors.textSecondary(context),
  };

  static String kindLabel(BuildContext context, String kind) => switch (kind) {
    'local' => AppLanguage.tr(context, 'Local'),
    'remote' => AppLanguage.tr(context, 'Remote'),
    _ => AppLanguage.tr(context, 'Dynamic'),
  };
}

class _JumpCard extends StatelessWidget {
  final JumpChain chain;

  const _JumpCard({required this.chain});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return SizedBox(
      height: 66,
      child: XtCard(
        key: ValueKey('jump-${chain.sessionId}'),
        padding: const EdgeInsets.symmetric(horizontal: 14),
        radius: XtRadius.cardLarge,
        child: Row(
          children: [
            Container(
              width: 40,
              height: 40,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: XtColors.selected(context),
                borderRadius: BorderRadius.circular(XtRadius.menu),
              ),
              child: Icon(
                LucideIcons.route,
                size: 18,
                color: XtColors.textPrimary(context),
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    chain.sessionName,
                    style: theme.typography.semiBold.copyWith(
                      fontSize: 14,
                      color: XtColors.textPrimary(context),
                    ),
                  ),
                  const SizedBox(height: 3),
                  Text(
                    chain.path,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontSize: 12,
                      color: XtColors.textTertiary(context),
                    ),
                  ),
                ],
              ),
            ),
            Text(
              chain.jumps.length == 1
                  ? AppLanguage.tr(context, 'via 1 jump')
                  : AppLanguage.tr(context, 'via {count} jumps', {
                      'count': chain.jumps.length,
                    }),
              style: TextStyle(
                fontSize: 11,
                color: XtColors.textTertiary(context),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _RuleCard extends StatefulWidget {
  final ForwardRule rule;
  final VoidCallback onEdit;
  final VoidCallback onDelete;

  const _RuleCard({
    required this.rule,
    required this.onEdit,
    required this.onDelete,
  });

  @override
  State<_RuleCard> createState() => _RuleCardState();
}

class _RuleCardState extends State<_RuleCard> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final rule = widget.rule;
    final color = _ForwardsScreenState.kindColor(context, rule.kind);
    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: SizedBox(
        height: 76,
        child: XtCard(
          key: ValueKey('forward-${rule.sessionId}-${rule.index}'),
          padding: const EdgeInsets.symmetric(horizontal: 14),
          radius: XtRadius.cardLarge,
          interactive: true,
          child: Row(
            children: [
              Container(
                width: 44,
                height: 44,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: Color.alphaBlend(
                    color.withValues(alpha: 0.16),
                    XtColors.surface(context),
                  ),
                  borderRadius: BorderRadius.circular(XtRadius.menu),
                ),
                child: Icon(LucideIcons.arrowLeftRight, size: 20, color: color),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Flexible(
                          child: Text(
                            rule.name.isEmpty ? rule.bind : rule.name,
                            overflow: TextOverflow.ellipsis,
                            style: theme.typography.semiBold.copyWith(
                              fontSize: 15,
                              color: XtColors.textPrimary(context),
                            ),
                          ),
                        ),
                        const SizedBox(width: 8),
                        XtBadge(
                          label: _ForwardsScreenState.kindLabel(
                            context,
                            rule.kind,
                          ),
                        ),
                      ],
                    ),
                    const SizedBox(height: 3),
                    Text(
                      rule.target.isEmpty
                          ? '${rule.bind}  ·  ${rule.sessionName}'
                          : '${rule.bind}  →  ${rule.target}  ·  ${rule.sessionName}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: 12.5,
                        color: XtColors.textSecondary(context),
                      ),
                    ),
                  ],
                ),
              ),
              if (_hover)
                Row(
                  children: [
                    XtActionButton(
                      icon: LucideIcons.pencil,
                      onPressed: widget.onEdit,
                      height: 30,
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                      tooltip: AppLanguage.tr(context, 'Edit rule'),
                    ),
                    const SizedBox(width: 4),
                    XtActionButton(
                      icon: LucideIcons.trash2,
                      variant: XtActionVariant.danger,
                      onPressed: widget.onDelete,
                      height: 30,
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                      tooltip: AppLanguage.tr(context, 'Delete rule'),
                    ),
                  ],
                ),
            ],
          ),
        ),
      ),
    );
  }
}

class ForwardEditor extends StatefulWidget {
  final ForwardRule? rule;
  final List<Host> hosts;
  final Future<String?> Function({
    required String sessionId,
    required int index,
    required String kind,
    required String name,
    required String bindAddr,
    required String bindPort,
    required String host,
    required String hostPort,
  })
  onSave;
  final Future<String?> Function(ForwardRule rule)? onDelete;
  final VoidCallback onClose;

  const ForwardEditor({
    super.key,
    required this.rule,
    required this.hosts,
    required this.onSave,
    required this.onDelete,
    required this.onClose,
  });

  @override
  State<ForwardEditor> createState() => _ForwardEditorState();
}

class _ForwardEditorState extends State<ForwardEditor> {
  late String _sessionId =
      widget.rule?.sessionId ??
      (widget.hosts.isEmpty ? '' : widget.hosts.first.id);
  late String _kind = widget.rule?.kind ?? 'local';
  late final TextEditingController _name = TextEditingController(
    text: widget.rule?.name ?? '',
  );
  late final TextEditingController _bindAddr = TextEditingController(
    text: widget.rule?.bindAddr ?? '127.0.0.1',
  );
  late final TextEditingController _bindPort = TextEditingController(
    text: widget.rule == null ? '' : '${widget.rule!.bindPort}',
  );
  late final TextEditingController _host = TextEditingController(
    text: widget.rule?.host ?? '',
  );
  late final TextEditingController _hostPort = TextEditingController(
    text: widget.rule == null || widget.rule!.hostPort == 0
        ? ''
        : '${widget.rule!.hostPort}',
  );
  bool _saving = false;
  bool _deleting = false;
  String? _error;

  @override
  void dispose() {
    _name.dispose();
    _bindAddr.dispose();
    _bindPort.dispose();
    _host.dispose();
    _hostPort.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final port = int.tryParse(_bindPort.text.trim());
    if (port == null || port < 1 || port > 65535) {
      setState(() => _error = 'Enter a valid listen port (1-65535).');
      return;
    }
    if (_kind != 'dynamic') {
      final targetPort = int.tryParse(_hostPort.text.trim());
      if (_host.text.trim().isEmpty ||
          targetPort == null ||
          targetPort < 1 ||
          targetPort > 65535) {
        setState(() => _error = 'Enter a target host and a valid target port.');
        return;
      }
    }
    setState(() {
      _saving = true;
      _error = null;
    });
    final error = await widget.onSave(
      sessionId: _sessionId,
      index: widget.rule?.index ?? -1,
      kind: _kind,
      name: _name.text.trim(),
      bindAddr: _bindAddr.text.trim(),
      bindPort: '$port',
      host: _kind == 'dynamic' ? '' : _host.text.trim(),
      hostPort: _kind == 'dynamic' ? '' : _hostPort.text.trim(),
    );
    if (!mounted) return;
    setState(() => _saving = false);
    if (error != null) {
      setState(() => _error = error);
    } else {
      widget.onClose();
    }
  }

  Future<void> _delete() async {
    final delete = widget.onDelete;
    if (delete == null) return;
    setState(() {
      _deleting = true;
      _error = null;
    });
    final error = await delete(widget.rule!);
    if (!mounted) return;
    setState(() => _deleting = false);
    if (error != null) setState(() => _error = error);
  }

  @override
  Widget build(BuildContext context) {
    return XtCard(
      margin: const EdgeInsets.fromLTRB(0, 8, 8, 8),
      padding: EdgeInsets.zero,
      border: false,
      radius: XtRadius.card,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          XtPanelHeader(
            title: AppLanguage.tr(
              context,
              widget.rule == null ? 'New rule' : 'Forwarding rule',
            ),
            height: 56,
            padding: const EdgeInsets.symmetric(horizontal: 15),
            actions: [
              if (widget.rule != null)
                XtActionButton(
                  key: const ValueKey('forward-delete'),
                  icon: _deleting
                      ? LucideIcons.loaderCircle
                      : LucideIcons.trash2,
                  variant: XtActionVariant.danger,
                  height: 28,
                  padding: const EdgeInsets.symmetric(horizontal: 7),
                  tooltip: AppLanguage.tr(context, 'Delete rule'),
                  onPressed: _deleting ? null : _delete,
                ),
              const SizedBox(width: 6),
              XtActionButton(
                key: const ValueKey('forward-save'),
                icon: _saving ? LucideIcons.loaderCircle : LucideIcons.check,
                variant: XtActionVariant.secondary,
                height: 28,
                padding: const EdgeInsets.symmetric(horizontal: 7),
                tooltip: AppLanguage.tr(context, 'Save rule'),
                onPressed: _saving ? null : _save,
              ),
              const SizedBox(width: 6),
              XtActionButton(
                icon: LucideIcons.x,
                height: 28,
                padding: const EdgeInsets.symmetric(horizontal: 7),
                tooltip: AppLanguage.tr(context, 'Close'),
                onPressed: widget.onClose,
              ),
            ],
          ),
          Expanded(
            child: ListView(
              padding: const EdgeInsets.fromLTRB(15, 15, 15, 15),
              children: [
                if (_error != null) ...[
                  Container(
                    padding: const EdgeInsets.all(10),
                    decoration: BoxDecoration(
                      color: XtColors.dangerTint(context),
                      borderRadius: BorderRadius.circular(XtRadius.field),
                    ),
                    child: Text(
                      AppLanguage.tr(context, _error!),
                      style: TextStyle(
                        fontSize: 12,
                        color: XtColors.dangerText(context),
                      ),
                    ),
                  ),
                  const SizedBox(height: 12),
                ],
                Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    XtFieldLabel(AppLanguage.tr(context, 'Host')),
                    XtSelectField<String>(
                      key: const ValueKey('forward-host'),
                      value: _sessionId,
                      options: {
                        for (final host in widget.hosts)
                          host.id: '${host.label} · ${host.address}',
                      },
                      onChanged: (value) => setState(() => _sessionId = value),
                    ),
                  ],
                ),
                const SizedBox(height: 10),
                Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    XtFieldLabel(AppLanguage.tr(context, 'Type')),
                    XtSelectField<String>(
                      key: const ValueKey('forward-kind'),
                      value: _kind,
                      options: const {
                        'local': 'Local',
                        'remote': 'Remote',
                        'dynamic': 'Dynamic (SOCKS5)',
                      },
                      onChanged: (value) => setState(() => _kind = value),
                    ),
                  ],
                ),
                const SizedBox(height: 10),
                XtField(
                  key: const ValueKey('forward-name'),
                  controller: _name,
                  placeholder: AppLanguage.tr(context, 'Name (optional)'),
                ),
                const SizedBox(height: 10),
                XtField(
                  key: const ValueKey('forward-bind-addr'),
                  controller: _bindAddr,
                  placeholder: AppLanguage.tr(context, 'Listen address'),
                ),
                const SizedBox(height: 10),
                XtField(
                  key: const ValueKey('forward-bind-port'),
                  controller: _bindPort,
                  keyboardType: TextInputType.number,
                  placeholder: AppLanguage.tr(context, 'Listen port'),
                ),
                if (_kind != 'dynamic') ...[
                  const SizedBox(height: 10),
                  XtField(
                    key: const ValueKey('forward-target-host'),
                    controller: _host,
                    placeholder: AppLanguage.tr(context, 'Target host'),
                  ),
                  const SizedBox(height: 10),
                  XtField(
                    key: const ValueKey('forward-target-port'),
                    controller: _hostPort,
                    keyboardType: TextInputType.number,
                    placeholder: AppLanguage.tr(context, 'Target port'),
                  ),
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// One live tunnel: what it binds locally and where it lands, with the
/// session's own status text.
class _TunnelCard extends StatelessWidget {
  final RuntimeTunnel tunnel;

  const _TunnelCard({required this.tunnel});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = tunnel.active
        ? XtColors.accent(context)
        : XtColors.danger(context);
    return XtCard(
      padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
      radius: XtRadius.field,
      child: Row(
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(color: color, shape: BoxShape.circle),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  tunnel.name.isEmpty
                      ? AppLanguage.tr(context, '{kind} tunnel', {
                          'kind': _ForwardsScreenState.kindLabel(
                            context,
                            tunnel.kind,
                          ),
                        })
                      : tunnel.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: theme.typography.medium.copyWith(
                    fontSize: 13,
                    color: XtColors.textPrimary(context),
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  '${tunnel.bindAddr}:${tunnel.bindPort} → '
                  '${tunnel.host}:${tunnel.hostPort}',
                  style: TextStyle(
                    fontFamily: 'Consolas',
                    fontFamilyFallback: const ['Menlo', 'Courier New'],
                    fontSize: 11,
                    color: XtColors.textTertiary(context),
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 10),
          Text(
            tunnel.status,
            style: TextStyle(
              fontSize: 11,
              color: XtColors.textTertiary(context),
            ),
          ),
        ],
      ),
    );
  }
}
