/// One host card: the OS glyph tile, the name over the connection target, and
/// an edit pencil revealed on hover.
library;

import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:simple_icons/simple_icons.dart';

import '../../data/host_os_registry.dart';
import '../../data/hosts.dart';
import '../../l10n/app_language.dart';
import '../../theme/app_theme.dart';
import '../../widgets/primitives.dart';

/// The distro glyph for a host. The theme is monochrome, so the brand glyph
/// (Simple Icons) inside the neutral tile is the only differentiator.
IconData osGlyphFor(HostOs os) => switch (os) {
  HostOs.unknown => LucideIcons.server,
  HostOs.debian => SimpleIcons.debian,
  HostOs.ubuntu => SimpleIcons.ubuntu,
  HostOs.arch => SimpleIcons.archlinux,
  HostOs.linux => SimpleIcons.linux,
  HostOs.windows => LucideIcons.appWindow,
  HostOs.apple => SimpleIcons.apple,
  HostOs.android => SimpleIcons.android,
};

/// The glyph the tile shows for [host]: a detected distro logo when
/// [hostOsRegistry] reported one, otherwise the neutral server glyph.
IconData osGlyphForHost(Host host) {
  final detected = hostOsRegistry.of(host.id);
  return osGlyphFor(detected ?? host.os);
}

/// The square OS glyph tile shared by the host card and the details drawer.
class OsChipTile extends StatelessWidget {
  final Host host;
  final double size;
  final double iconSize;

  const OsChipTile({
    super.key,
    required this.host,
    this.size = 40,
    this.iconSize = 22,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      width: size,
      height: size,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: XtColors.selected(context),
        borderRadius: BorderRadius.circular(XtRadius.card),
      ),
      child: Icon(
        osGlyphForHost(host),
        size: iconSize,
        color: XtColors.textPrimary(context),
      ),
    );
  }
}

class HostCard extends StatefulWidget {
  final Host host;
  final bool selected;
  final VoidCallback onTap;
  final VoidCallback onEdit;
  final bool grid;

  const HostCard.grid({
    super.key,
    required this.host,
    required this.selected,
    required this.onTap,
    required this.onEdit,
  }) : grid = true;

  const HostCard.row({
    super.key,
    required this.host,
    required this.selected,
    required this.onTap,
    required this.onEdit,
  }) : grid = false;

  @override
  State<HostCard> createState() => _HostCardState();
}

class _HostCardState extends State<HostCard> {
  bool _hovering = false;

  /// The card's second line: the connection target, not the metadata that the
  /// details drawer already lists (protocol/user/group repeated on every card).
  String _subtitle(BuildContext context) {
    final host = widget.host;
    if (host.protocol == 'local') {
      return AppLanguage.tr(context, 'Local terminal');
    }
    final detail = host.detail.trim();
    if (detail.isEmpty) return host.address;
    return host.protocol == 'ssh' ? detail : '${host.protocol} · $detail';
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final host = widget.host;
    final card = XtCard(
      padding: const EdgeInsets.symmetric(horizontal: 15),
      radius: XtRadius.cardLarge,
      color: widget.selected ? XtColors.selected(context) : null,
      interactive: true,
      onTap: widget.onTap,
      child: SizedBox(
        height: 60,
        child: Row(
          children: [
            OsChipTile(host: host),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    host.label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.typography.semiBold.copyWith(
                      fontSize: 16,
                      color: XtColors.textPrimary(context),
                    ),
                  ),
                  const SizedBox(height: 1),
                  Text(
                    _subtitle(context),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.typography.normal.copyWith(
                      fontSize: 13,
                      color: XtColors.textSecondary(context),
                    ),
                  ),
                ],
              ),
            ),
            // The edit pencil is the per-card quick action, revealed on hover.
            AnimatedOpacity(
              duration: XtMotion.micro,
              curve: XtMotion.easeOut,
              opacity: _hovering ? 1 : 0,
              child: XtIconButton(
                key: ValueKey('edit-${host.id}'),
                icon: LucideIcons.pencil,
                onPressed: widget.onEdit,
                size: 28,
                iconSize: 13,
              ),
            ),
          ],
        ),
      ),
    );
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hovering = true),
      onExit: (_) => setState(() => _hovering = false),
      child: card,
    );
  }
}
