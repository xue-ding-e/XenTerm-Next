/// Known Hosts — the core's TOFU `known_hosts` store. Lists the real remembered
/// keys (host:port, algorithm, SHA256 fingerprint) and removes entries; no
/// fixture rows.
library;

import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../../bridge/native_bridge.dart';
import '../../l10n/app_language.dart';
import '../../theme/app_field.dart';
import '../../theme/app_theme.dart';
import '../../widgets/primitives.dart';
import '../../widgets/toolbar.dart';

typedef KnownHostLoader = Future<List<KnownHost>> Function();
typedef KnownHostDeleter = Future<CoreWriteResult> Function(String id);

class KnownHostsScreen extends StatefulWidget {
  final KnownHostLoader? loadHosts;
  final KnownHostDeleter? deleteHost;

  const KnownHostsScreen({super.key, this.loadHosts, this.deleteHost});

  @override
  State<KnownHostsScreen> createState() => _KnownHostsScreenState();
}

class _KnownHostsScreenState extends State<KnownHostsScreen> {
  List<KnownHost> _hosts = const [];
  bool _loading = true;
  String? _error;
  bool _searching = false;
  String _query = '';
  final TextEditingController _search = TextEditingController();

  @override
  void initState() {
    super.initState();
    _reload();
  }

  @override
  void dispose() {
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
      final hosts = await (widget.loadHosts ?? loadKnownHostsFromCore)();
      if (!mounted) return;
      setState(() {
        _hosts = hosts;
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

  Future<void> _remove(KnownHost host) async {
    try {
      final result = await (widget.deleteHost ?? deleteKnownHostFromCore)(
        host.id,
      );
      if (!result.ok) {
        if (!mounted) return;
        setState(() => _error = result.error ?? 'Could not remove the key');
        return;
      }
      await _reload(silent: true);
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = '$error');
    }
  }

  List<KnownHost> get _filtered {
    final query = _query.trim().toLowerCase();
    if (query.isEmpty) return _hosts;
    return _hosts
        .where(
          (h) =>
              h.id.toLowerCase().contains(query) ||
              h.keyType.toLowerCase().contains(query) ||
              h.fingerprint.toLowerCase().contains(query),
        )
        .toList();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
          child: Row(
            children: [
              const Spacer(),
              Row(
                children: [
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
            ],
          ),
        ),
        if (_searching)
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 4),
            child: XtField(
              key: const ValueKey('known-host-search'),
              controller: _search,
              autofocus: true,
              onChanged: (value) => setState(() => _query = value),
              placeholder: AppLanguage.tr(context, 'Search known hosts'),
            ),
          ),
        SectionTitle(text: AppLanguage.tr(context, 'Known Hosts')),
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
          child: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 760),
              child: _body(theme),
            ),
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
            AppLanguage.tr(context, 'Loading known hosts…'),
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
    final hosts = _filtered;
    if (hosts.isEmpty) {
      return XtEmptyState(
        icon: LucideIcons.fingerprint,
        title: AppLanguage.tr(
          context,
          _hosts.isEmpty ? 'No known hosts yet' : 'No matches',
        ),
        message: AppLanguage.tr(
          context,
          _hosts.isEmpty
              ? 'Host keys are remembered after you accept them on connect.'
              : 'Try a different search.',
        ),
      );
    }
    return Column(
      children: [
        for (final host in hosts)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: KnownHostCard(host: host, onRemove: () => _remove(host)),
          ),
      ],
    );
  }
}

class KnownHostCard extends StatefulWidget {
  final KnownHost host;
  final VoidCallback onRemove;

  const KnownHostCard({super.key, required this.host, required this.onRemove});

  @override
  State<KnownHostCard> createState() => _KnownHostCardState();
}

class _KnownHostCardState extends State<KnownHostCard> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final fingerprint = widget.host.fingerprint.isEmpty
        ? AppLanguage.tr(context, 'unreadable key entry')
        : widget.host.fingerprint;
    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: SizedBox(
        height: 76,
        child: XtCard(
          key: ValueKey('known-host-${widget.host.id}'),
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
                  color: XtColors.sunken(context),
                  borderRadius: BorderRadius.circular(XtRadius.menu),
                ),
                child: Icon(
                  LucideIcons.shieldCheck,
                  size: 20,
                  color: XtColors.textPrimary(context),
                ),
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
                            widget.host.id,
                            overflow: TextOverflow.ellipsis,
                            style: theme.typography.semiBold.copyWith(
                              fontSize: 15,
                              color: XtColors.textPrimary(context),
                            ),
                          ),
                        ),
                        const SizedBox(width: 8),
                        XtBadge(label: _algorithmLabel(widget.host.keyType)),
                      ],
                    ),
                    const SizedBox(height: 3),
                    Text(
                      fingerprint,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontFamily: 'Consolas',
                        fontFamilyFallback: const ['Menlo', 'Courier New'],
                        fontSize: 12,
                        color: XtColors.textSecondary(context),
                      ),
                    ),
                  ],
                ),
              ),
              if (_hover)
                XtActionButton(
                  key: ValueKey('known-host-remove-${widget.host.id}'),
                  icon: LucideIcons.trash2,
                  variant: XtActionVariant.danger,
                  tooltip: AppLanguage.tr(context, 'Forget this key'),
                  height: 30,
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  onPressed: widget.onRemove,
                ),
            ],
          ),
        ),
      ),
    );
  }

  static String _algorithmLabel(String type) {
    final lower = type.toLowerCase();
    if (lower.contains('ed25519')) return 'ED25519';
    if (lower.contains('rsa')) return 'RSA';
    if (lower.contains('ecdsa')) return 'ECDSA';
    return type.isEmpty ? 'KEY' : type.toUpperCase();
  }
}
