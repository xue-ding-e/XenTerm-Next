/// Shared building blocks for the host drawers.
///
/// The SSH editor and the serial editor render the same drawer surface; before
/// this file they had drifted apart (uppercase micro labels vs 15px section
/// titles, different card paddings), which read as two different products.
/// Both now compose these widgets so the typography, colours and spacing stay
/// identical.
library;

import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../../l10n/app_language.dart';
import '../../theme/app_field.dart';
import '../../theme/app_theme.dart';
import '../../widgets/primitives.dart';

/// One surface card with evenly spaced rows (10px between rows).
class HostFormCard extends StatelessWidget {
  final List<Widget> children;
  final EdgeInsetsGeometry padding;

  const HostFormCard({
    super.key,
    required this.children,
    this.padding = const EdgeInsets.fromLTRB(20, 15, 20, 20),
  });

  @override
  Widget build(BuildContext context) {
    final spaced = <Widget>[];
    for (var i = 0; i < children.length; i++) {
      if (i > 0) spaced.add(const SizedBox(height: 10));
      spaced.add(children[i]);
    }
    return XtCard(
      padding: padding,
      radius: XtRadius.field,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: spaced,
      ),
    );
  }
}

/// A 15px semibold title inside a form card.
class HostSectionTitle extends StatelessWidget {
  final String text;

  const HostSectionTitle(this.text, {super.key});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Text(
      text,
      style: theme.typography.medium.copyWith(
        fontSize: 15,
        color: XtColors.textPrimary(context),
      ),
    );
  }
}

/// The one drawer input: an [XtField] with an optional leading icon.
class HostPanelField extends StatelessWidget {
  final String placeholder;
  final TextEditingController controller;
  final IconData? leading;
  final double height;
  final bool obscure;

  const HostPanelField({
    super.key,
    required this.placeholder,
    required this.controller,
    this.leading,
    this.height = XtMetrics.controlHeight,
    this.obscure = false,
  });

  @override
  Widget build(BuildContext context) {
    return XtField(
      placeholder: placeholder,
      controller: controller,
      height: height,
      obscure: obscure,
      leading: leading == null
          ? null
          : Icon(leading, size: 14, color: XtColors.textDisabled(context)),
    );
  }
}

/// The drawer title bar: the shared panel header with an optional leading glyph
/// and trailing actions.
class HostDrawerHeader extends StatelessWidget {
  final String title;
  final IconData? leading;
  final List<Widget> actions;

  const HostDrawerHeader({
    super.key,
    required this.title,
    this.leading,
    this.actions = const [],
  });

  @override
  Widget build(BuildContext context) {
    return XtPanelHeader(title: title, leading: leading, actions: actions);
  }
}

/// The ⋯ menu with a destructive "Remove host" entry, shared by every drawer
/// kind (SSH/Telnet/local and serial) so deleting a connection works the same
/// way regardless of its protocol.
class HostRemoveMenu extends StatefulWidget {
  final Future<void> Function() onDelete;

  const HostRemoveMenu({super.key, required this.onDelete});

  @override
  State<HostRemoveMenu> createState() => _HostRemoveMenuState();
}

class _HostRemoveMenuState extends State<HostRemoveMenu> {
  final OverlayPortalController _menu = OverlayPortalController();
  final LayerLink _link = LayerLink();

  void _toggle() => setState(() {
    if (_menu.isShowing) {
      _menu.hide();
    } else {
      _menu.show();
    }
  });

  Widget _menuChild(BuildContext context) {
    return Stack(
      children: [
        Positioned.fill(
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: () => _menu.hide(),
            child: const ColoredBox(color: Colors.transparent),
          ),
        ),
        CompositedTransformFollower(
          link: _link,
          targetAnchor: Alignment.bottomRight,
          followerAnchor: Alignment.topRight,
          offset: const Offset(0, 6),
          showWhenUnlinked: false,
          child: Align(
            alignment: Alignment.topRight,
            child: XtMenuPopup(
              width: 180,
              children: [
                XtMenuEntry(
                  icon: LucideIcons.trash2,
                  label: AppLanguage.tr(context, 'Remove host'),
                  danger: true,
                  onTap: () {
                    _menu.hide();
                    widget.onDelete();
                  },
                ),
              ],
            ),
          ),
        ),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    return OverlayPortal(
      controller: _menu,
      overlayChildBuilder: _menuChild,
      child: CompositedTransformTarget(
        link: _link,
        child: XtIconButton(
          key: const ValueKey('host-remove-menu'),
          icon: LucideIcons.ellipsis,
          size: 28,
          iconSize: 15,
          onPressed: _toggle,
        ),
      ),
    );
  }
}
