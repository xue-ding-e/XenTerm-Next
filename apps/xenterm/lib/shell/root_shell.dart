/// The root shell: a two-column spine — the labelled navigation rail (182px)
/// and the content area. The rail carries the screens; Settings anchors its
/// bottom, keeping app-wide actions apart from navigation.
library;

import 'package:flutter/material.dart' as material;
import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../theme/app_theme.dart';
import '../widgets/lucide_extra_icons.dart';
import '../widgets/primitives.dart';
import 'title_bar.dart';
import '../l10n/app_language.dart';
import '../screens/hosts/hosts_screen.dart';
import '../screens/forwards/forwards_screen.dart';
import '../screens/known_hosts/known_hosts_screen.dart';
import '../screens/snippets/snippets_screen.dart';
import '../screens/keychain/keychain_screen.dart';
import '../screens/logs/logs_screen.dart';
import '../screens/settings/settings_screen.dart';
import '../screens/terminal/terminal_screen.dart';
import '../data/hosts.dart';
import '../terminal/core_settings.dart';
import '../terminal/terminal_command_bus.dart';
import '../theme/app_fonts.dart';

enum NavDestination {
  hosts,
  keys,
  forwards,
  snippets,
  knownHosts,
  logs,
  settings,
}

// Every rail screen is backed by real core data (sessions, quick commands,
// known hosts, forwards, keys, transcripts, settings).

class RootShell extends StatefulWidget {
  final bool lightTheme;
  final ValueChanged<bool> onThemeChanged;
  final AppFonts fonts;
  final CoreSettingsController settings;
  final Future<List<Host>> Function()? hostsLoader;

  const RootShell({
    super.key,
    required this.lightTheme,
    required this.onThemeChanged,
    required this.fonts,
    required this.settings,
    this.hostsLoader,
  });

  @override
  State<RootShell> createState() => _RootShellState();
}

class _RootShellState extends State<RootShell> {
  NavDestination _active = NavDestination.hosts;
  final List<Host> _openHosts = <Host>[];
  String? _activeHostId;
  bool _terminalSftp = false;
  bool _railOpen = true;
  final TerminalCommandBus _commands = TerminalCommandBus();
  final ValueNotifier<int> _newHostRequest = ValueNotifier<int>(0);

  void _openHost(Host host) {
    setState(() {
      if (!_openHosts.any((h) => h.id == host.id)) _openHosts.add(host);
      _activeHostId = host.id;
      // The terminal gets the full width; the hamburger brings the rail back.
      _railOpen = false;
    });
  }

  /// The title bar's + opens a fresh editor on the Hosts screen.
  void _requestNewHost() {
    setState(() {
      _active = NavDestination.hosts;
      _activeHostId = null;
      _terminalSftp = false;
    });
    // HostsScreen may be built by this same frame; notify after it mounts.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _newHostRequest.value++;
    });
  }

  @override
  void dispose() {
    _newHostRequest.dispose();
    _commands.dispose();
    super.dispose();
  }

  void _closeHost(String id) {
    setState(() {
      _openHosts.removeWhere((h) => h.id == id);
      if (_activeHostId == id) {
        _activeHostId = null;
        _terminalSftp = false;
        if (_openHosts.isEmpty) _railOpen = true;
      }
    });
  }

  static const List<(NavDestination, String, IconData)> _destinations = [
    (NavDestination.hosts, 'Home', LucideIcons.server),
    (NavDestination.keys, 'Keychain', LucideIcons.keyRound),
    (NavDestination.forwards, 'Tunnels', LucideIcons.arrowLeftRight),
    (NavDestination.snippets, 'Snippets', LucideIcons.braces),
    (NavDestination.knownHosts, 'Known Hosts', LucideIcons.shieldCheck),
    (NavDestination.logs, 'Logs', LucideIcons.circleAlert),
  ];

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: XtColors.page(context),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // The title bar repaints on tab hover/close affordances; keeping it
          // in its own layer stops those frames from touching the workspace.
          RepaintBoundary(
            child: TitleBar(
              openHosts: _openHosts,
              activeHostId: _activeHostId,
              onSelectHost: (id) => setState(() {
                _activeHostId = id;
                _terminalSftp = false;
              }),
              onCloseHost: _closeHost,
              onNewTab: _requestNewHost,
              showSftp: _terminalSftp,
              onToggleSftp: () => setState(() => _terminalSftp = !_terminalSftp),
              onToggleSidebar: () => setState(() => _railOpen = !_railOpen),
            ),
          ),
          Expanded(
            child: Row(
              children: [
                // Collapsible rail: fixed-width content clipped by an
                // animated 0⇄182px box (no squashing mid-animation). The rail
                // is its own layer: hover washes on the entries never repaint
                // the workspace beside it.
                RepaintBoundary(
                  child: ClipRect(
                    child: AnimatedContainer(
                      duration: XtMotion.micro,
                      curve: XtMotion.easeOut,
                      width: _railOpen ? XtMetrics.navRailWidth : 0,
                      child: OverflowBox(
                        alignment: Alignment.centerLeft,
                        minWidth: XtMetrics.navRailWidth,
                        maxWidth: XtMetrics.navRailWidth,
                        child: _NavRail(
                          active: _active,
                          destinations: _destinations,
                          onSelect: (d) => setState(() {
                            _active = d;
                            // Choosing a screen in the rail parks the session
                            // tabs; clicking a tab in the title bar brings them
                            // back.
                            _activeHostId = null;
                            _terminalSftp = false;
                          }),
                        ),
                      ),
                    ),
                  ),
                ),
                Expanded(
                  // Every open workspace stays mounted (offstage when not in
                  // front), so switching session tabs never reconnects and
                  // background PTYs keep running. Their UI polling is paused
                  // through `active`, so a hidden tab costs no frames.
                  child: Stack(
                    children: [
                      for (final host in _openHosts)
                        Positioned.fill(
                          child: Offstage(
                            offstage: host.id != _activeHostId,
                            child: TerminalScreen(
                              key: ValueKey('workspace-${host.id}'),
                              host: host,
                              fonts: widget.fonts,
                              settings: widget.settings,
                              showSftp:
                                  host.id == _activeHostId && _terminalSftp,
                              commandBus: _commands,
                              active: host.id == _activeHostId,
                            ),
                          ),
                        ),
                      if (_activeHostId == null)
                        Positioned.fill(child: _screenFor(_active)),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _screenFor(NavDestination destination) {
    return switch (destination) {
      NavDestination.hosts => HostsScreen(
        onConnect: _openHost,
        loadHosts: widget.hostsLoader,
        newHostRequest: _newHostRequest,
      ),
      NavDestination.keys => const KeychainScreen(),
      NavDestination.logs => const LogsScreen(),
      NavDestination.settings => SettingsScreen(
        lightTheme: widget.lightTheme,
        onThemeChanged: widget.onThemeChanged,
        fonts: widget.fonts,
        settings: widget.settings,
      ),
      NavDestination.forwards => const ForwardsScreen(),
      NavDestination.snippets => const SnippetsScreen(),
      NavDestination.knownHosts => const KnownHostsScreen(),
    };
  }
}

/// The labelled navigation rail: 36px entries, selected = chip, hover = wash.
/// Settings sits at the bottom.
class _NavRail extends StatelessWidget {
  final NavDestination active;
  final List<(NavDestination, String, IconData)> destinations;
  final ValueChanged<NavDestination> onSelect;

  const _NavRail({
    required this.active,
    required this.destinations,
    required this.onSelect,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      color: XtColors.chrome(context),
      padding: const EdgeInsets.symmetric(vertical: 8, horizontal: 10),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final (destination, title, icon) in destinations)
            _RailItem(
              icon: icon,
              glyphBuilder: switch (destination) {
                NavDestination.knownHosts => (
                  size,
                  color,
                ) => FingerprintPatternIcon(size: size, color: color),
                NavDestination.logs => (size, color) => ClipboardClockIcon(
                  size: size,
                  color: color,
                ),
                _ => null,
              },
              label: AppLanguage.tr(context, title),
              selected: destination == active,
              onTap: () => onSelect(destination),
            ),
          const Spacer(),
          const XtDivider(),
          const SizedBox(height: 4),
          _RailItem(
            icon: material.Icons.settings_outlined,
            label: AppLanguage.tr(context, 'Settings'),
            selected: active == NavDestination.settings,
            onTap: () => onSelect(NavDestination.settings),
          ),
        ],
      ),
    );
  }
}

class _RailItem extends StatelessWidget {
  final IconData icon;

  /// Overrides the font glyph (for icons newer than the bundled set).
  final Widget Function(double size, Color color)? glyphBuilder;
  final String label;
  final bool selected;
  final VoidCallback onTap;

  const _RailItem({
    required this.icon,
    this.glyphBuilder,
    required this.label,
    required this.selected,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final foreground = selected
        ? XtColors.textPrimary(context)
        : XtColors.textSecondary(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: XtHoverRow(
        onTap: onTap,
        selected: selected,
        height: 36,
        padding: const EdgeInsets.symmetric(horizontal: 12),
        child: Row(
          children: [
            glyphBuilder?.call(16, foreground) ??
                Icon(icon, size: 16, color: foreground),
            const SizedBox(width: 12),
            Expanded(
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: theme.typography.normal.copyWith(
                  fontSize: 15,
                  color: foreground,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
