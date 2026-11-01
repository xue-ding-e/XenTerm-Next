import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../../data/hosts.dart';
import '../../terminal/core_settings.dart';
import '../../terminal/terminal_command_bus.dart';
import '../../theme/app_fonts.dart';
import '../../theme/app_theme.dart';
import 'sftp_view.dart';
import 'terminal_view.dart';

/// One connection workspace: the live terminal (or its SFTP view) on the left,
/// the tools panel on the right. Session tabs live in the title bar.
class TerminalScreen extends StatefulWidget {
  final Host host;
  final AppFonts fonts;

  /// SFTP view is toggled from the title bar's SFTP tab.
  final bool showSftp;
  final TerminalCommandBus? commandBus;
  final CoreSettingsController? settings;

  /// Whether this workspace is the visible foreground tab of the shell.
  final bool active;

  const TerminalScreen({
    super.key,
    required this.host,
    required this.fonts,
    this.showSftp = false,
    this.commandBus,
    this.settings,
    this.active = true,
  });

  @override
  State<TerminalScreen> createState() => _TerminalScreenState();
}

class _TerminalScreenState extends State<TerminalScreen> {
  /// Once the SFTP panel is opened it stays mounted (offstage when hidden), so
  /// toggling back to the terminal does not drop the remote connection.
  bool _sftpOpened = false;

  @override
  void initState() {
    super.initState();
    _sftpOpened = widget.showSftp;
  }

  @override
  void didUpdateWidget(TerminalScreen oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.showSftp) _sftpOpened = true;
    // While the SFTP view covers the terminal, keys must not reach the hidden
    // PTY; the offstage terminal would otherwise keep primary focus.
    if (widget.showSftp && !oldWidget.showSftp) {
      FocusManager.instance.primaryFocus?.unfocus();
    }
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      color: XtColors.page(context),
      child: Stack(
        children: [
          Positioned.fill(
            child: Offstage(
              offstage: widget.showSftp,
              child: TerminalView(
                host: widget.host,
                fonts: widget.fonts,
                commandBus: widget.commandBus,
                settings: widget.settings,
                // Visible = this workspace is the front tab AND the SFTP view
                // is not covering the terminal. Hidden panes stop polling.
                active: widget.active && !widget.showSftp,
              ),
            ),
          ),
          if (_sftpOpened)
            Positioned.fill(
              child: Offstage(
                offstage: !widget.showSftp,
                child: SftpView(
                  host: widget.host,
                  settings: widget.settings,
                  active: widget.active && widget.showSftp,
                ),
              ),
            ),
        ],
      ),
    );
  }
}
