/// Serial connection details in the right-hand host drawer: pick a detected
/// port (or type one), set the line parameters, and optionally store the
/// console login some serial servers ask for. It uses the same drawer as the
/// SSH host editor, with the same header and form cards.
library;

import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../../bridge/native_bridge.dart';
import '../../data/hosts.dart';
import '../../l10n/app_language.dart';
import '../../theme/app_field.dart';
import '../../theme/app_theme.dart';
import '../../widgets/primitives.dart';
import 'host_form_widgets.dart';

/// The values the panel hands back; the screen owns persistence.
class SerialDraft {
  final String label;
  final String port;
  final String group;
  final int baudRate;
  final int dataBits;
  final int stopBits;
  final String parity;
  final String flowControl;
  final String username;
  final String password;

  const SerialDraft({
    required this.label,
    required this.port,
    required this.group,
    required this.baudRate,
    required this.dataBits,
    required this.stopBits,
    required this.parity,
    required this.flowControl,
    required this.username,
    required this.password,
  });
}

typedef SerialPortLoader = Future<List<SerialPortInfo>> Function();

class SerialDetailsPanel extends StatefulWidget {
  /// Existing serial host when editing; null creates a new session.
  final Host? host;
  final SerialPortLoader? loadPorts;
  final ValueChanged<SerialDraft> onSubmit;
  final VoidCallback onClose;

  /// Offered only while creating: switching the connection type back to SSH
  /// reopens the drawer with the SSH editor.
  final VoidCallback? onSwitchToSsh;

  /// True when the drawer came from the New host draft: submitting saves the
  /// session instead of opening a temporary tab.
  final bool saveMode;

  /// Group names already in use, for the Parent group picker.
  final List<String> groups;

  /// Persistence error from the screen; shown in the panel so the typed
  /// values are not lost when a save fails.
  final String? error;

  /// Deletes this saved serial session through the core. Absent while creating
  /// a draft (there is nothing stored to delete yet); provided for an existing
  /// host, exactly like the SSH/Telnet/local drawer's "Remove host" action.
  final Future<String?> Function(Host)? onDelete;

  const SerialDetailsPanel({
    super.key,
    this.host,
    this.loadPorts,
    required this.onSubmit,
    required this.onClose,
    this.onSwitchToSsh,
    this.saveMode = false,
    this.groups = const [],
    this.error,
    this.onDelete,
  });

  @override
  State<SerialDetailsPanel> createState() => _SerialDetailsPanelState();
}

class _SerialDetailsPanelState extends State<SerialDetailsPanel> {
  static const _baudRates = [
    1200,
    2400,
    4800,
    9600,
    19200,
    38400,
    57600,
    115200,
    230400,
    460800,
    921600,
  ];

  late final TextEditingController _label = TextEditingController(
    text: widget.host?.label ?? '',
  );
  bool _labelSeeded = false;
  late final TextEditingController _port = TextEditingController(
    text: widget.host?.address ?? '',
  );
  late final TextEditingController _user = TextEditingController(
    text: widget.host?.username ?? '',
  );
  final TextEditingController _password = TextEditingController();
  late int _baud = widget.host?.baudRate ?? 115200;
  late int _dataBits = widget.host?.dataBits ?? 8;
  late int _stopBits = widget.host?.stopBits ?? 1;
  late String _parity = widget.host?.parity ?? 'none';
  late String _group = widget.host?.group ?? '';
  late String _flow = widget.host?.flowControl ?? 'none';
  bool _advanced = false;
  bool _loadingPorts = true;
  List<SerialPortInfo> _ports = const [];
  String? _error;

  @override
  void initState() {
    super.initState();
    _refreshPorts();
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_labelSeeded) return;
    _labelSeeded = true;
    if (_label.text.isEmpty) {
      _label.text = AppLanguage.tr(context, 'Serial');
    }
  }

  @override
  void dispose() {
    _label.dispose();
    _port.dispose();
    _user.dispose();
    _password.dispose();
    super.dispose();
  }

  Future<void> _refreshPorts() async {
    setState(() => _loadingPorts = true);
    try {
      final ports = await (widget.loadPorts ?? loadSerialPortsFromCore)();
      if (!mounted) return;
      setState(() {
        _ports = ports;
        _loadingPorts = false;
        // A new serial session starts on the first attached device.
        if (_port.text.trim().isEmpty && ports.isNotEmpty) {
          _port.text = ports.first.name;
        }
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _loadingPorts = false;
        _ports = const [];
        _error = error is NativeBridgeError ? error.message : '$error';
      });
    }
  }

  void _submit() {
    final port = _port.text.trim();
    if (port.isEmpty) {
      setState(() => _error = 'Choose or type a serial port');
      return;
    }
    final label = _label.text.trim();
    widget.onSubmit(
      SerialDraft(
        label: label.isEmpty ? port : label,
        port: port,
        group: _group,
        baudRate: _baud,
        dataBits: _dataBits,
        stopBits: _stopBits,
        parity: _parity,
        flowControl: _flow,
        username: _user.text.trim(),
        password: _password.text,
      ),
    );
  }

  Future<void> _delete() async {
    final host = widget.host;
    final delete = widget.onDelete;
    if (host == null || delete == null) return;
    final error = await delete(host);
    if (mounted && error != null) setState(() => _error = error);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final editing = widget.host != null;
    return Container(
      color: XtColors.page(context),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          HostDrawerHeader(
            title: AppLanguage.tr(
              context,
              editing ? 'Serial Details' : 'Serial Connection',
            ),
            leading: LucideIcons.ethernetPort,
            actions: [
              // Saved serial sessions delete exactly like the SSH drawer;
              // drafts have nothing stored yet, so the menu is hidden.
              if (editing && widget.onDelete != null) ...[
                HostRemoveMenu(onDelete: _delete),
                const SizedBox(width: 4),
              ],
              XtIconButton(
                key: const ValueKey('serial-cancel'),
                icon: LucideIcons.arrowRightToLine,
                size: 28,
                iconSize: 16,
                onPressed: widget.onClose,
              ),
            ],
          ),
          Expanded(
            child: SingleChildScrollView(
              padding: const EdgeInsets.fromLTRB(15, 15, 15, 20),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (widget.host == null && widget.onSwitchToSsh != null) ...[
                    HostFormCard(
                      children: [
                        HostSectionTitle(
                          AppLanguage.tr(context, 'Connection type'),
                        ),
                        XtSelectField<String>(
                          key: const ValueKey('serial-kind'),
                          value: 'serial',
                          options: const {'ssh': 'SSH', 'serial': 'Serial'},
                          onChanged: (value) {
                            if (value == 'ssh') widget.onSwitchToSsh!();
                          },
                        ),
                      ],
                    ),
                    const SizedBox(height: 15),
                  ],
                  HostFormCard(
                    children: [
                      HostSectionTitle(AppLanguage.tr(context, 'Connection')),
                      HostPanelField(
                        key: const ValueKey('serial-label'),
                        controller: _label,
                        placeholder: AppLanguage.tr(context, 'Name'),
                      ),
                      Row(
                        children: [
                          Expanded(
                            child: Row(
                              children: [
                                Icon(
                                  LucideIcons.ethernetPort,
                                  size: 14,
                                  color: XtColors.textTertiary(context),
                                ),
                                const SizedBox(width: 6),
                                Text(
                                  AppLanguage.tr(context, 'Port'),
                                  style: TextStyle(
                                    fontSize: 13,
                                    color: XtColors.textSecondary(context),
                                  ),
                                ),
                              ],
                            ),
                          ),
                          XtActionButton(
                            key: const ValueKey('serial-refresh'),
                            icon: LucideIcons.refreshCw,
                            label: _loadingPorts
                                ? AppLanguage.tr(context, 'Scanning…')
                                : AppLanguage.tr(context, 'Refresh'),
                            height: 28,
                            onPressed: _refreshPorts,
                          ),
                        ],
                      ),
                      // Dropdown + free input: picking a detected port fills
                      // the field, but any device path can still be typed.
                      _PortComboField(
                        controller: _port,
                        ports: _ports,
                        loading: _loadingPorts,
                        onRefresh: _refreshPorts,
                      ),
                      HostSectionTitle(AppLanguage.tr(context, 'Parent group')),
                      XtSelectField<String>(
                        key: const ValueKey('serial-group'),
                        value: _group,
                        options: {
                          '': 'None',
                          for (final group in widget.groups) group: group,
                        },
                        onChanged: (value) =>
                            setState(() => _group = value),
                      ),
                      HostSectionTitle(AppLanguage.tr(context, 'Baud rate')),
                      XtSelectField<int>(
                        key: const ValueKey('serial-baud'),
                        value: _baud,
                        options: {
                          for (final rate in _baudRates) rate: '$rate',
                        },
                        onChanged: (value) => setState(() => _baud = value),
                      ),
                      GestureDetector(
                        key: const ValueKey('serial-advanced'),
                        onTap: () => setState(() => _advanced = !_advanced),
                        behavior: HitTestBehavior.opaque,
                        child: Row(
                          children: [
                            AnimatedRotation(
                              turns: _advanced ? 0.25 : 0,
                              duration: XtMotion.micro,
                              child: Icon(
                                LucideIcons.chevronRight,
                                size: 15,
                                color: XtColors.textTertiary(context),
                              ),
                            ),
                            const SizedBox(width: 6),
                            Text(
                              AppLanguage.tr(context, 'Advanced'),
                              style: theme.typography.medium.copyWith(
                                fontSize: 13,
                                color: XtColors.textPrimary(context),
                              ),
                            ),
                            const Spacer(),
                            Text(
                              '$_dataBits${_parityLetter(_parity)}$_stopBits',
                              style: TextStyle(
                                fontSize: 11,
                                color: XtColors.textTertiary(context),
                              ),
                            ),
                          ],
                        ),
                      ),
                      if (_advanced)
                        Column(
                          crossAxisAlignment: CrossAxisAlignment.stretch,
                          children: [
                            HostSectionTitle(
                              AppLanguage.tr(context, 'Data bits'),
                            ),
                            XtSelectField<int>(
                              key: const ValueKey('serial-data-bits'),
                              value: _dataBits,
                              options: const {5: '5', 6: '6', 7: '7', 8: '8'},
                              onChanged: (value) =>
                                  setState(() => _dataBits = value),
                            ),
                            const SizedBox(height: 12),
                            HostSectionTitle(
                              AppLanguage.tr(context, 'Stop bits'),
                            ),
                            XtSelectField<int>(
                              key: const ValueKey('serial-stop-bits'),
                              value: _stopBits,
                              options: const {1: '1', 2: '2'},
                              onChanged: (value) =>
                                  setState(() => _stopBits = value),
                            ),
                            const SizedBox(height: 12),
                            HostSectionTitle(AppLanguage.tr(context, 'Parity')),
                            XtSelectField<String>(
                              key: const ValueKey('serial-parity'),
                              value: _parity,
                              options: const {
                                'none': 'None',
                                'even': 'Even',
                                'odd': 'Odd',
                              },
                              onChanged: (value) =>
                                  setState(() => _parity = value),
                            ),
                            const SizedBox(height: 12),
                            HostSectionTitle(
                              AppLanguage.tr(context, 'Flow control'),
                            ),
                            XtSelectField<String>(
                              key: const ValueKey('serial-flow'),
                              value: _flow,
                              options: const {
                                'none': 'None',
                                'hardware': 'RTS/CTS',
                                'software': 'XON/XOFF',
                              },
                              onChanged: (value) =>
                                  setState(() => _flow = value),
                            ),
                            const SizedBox(height: 18),
                            HostSectionTitle(
                              AppLanguage.tr(context, 'Console login'),
                            ),
                            Text(
                              AppLanguage.tr(
                                context,
                                'Some serial servers ask for a login before the '
                                'shell. When set, the app answers those prompts.',
                              ),
                              style: TextStyle(
                                fontSize: 11,
                                color: XtColors.textTertiary(context),
                              ),
                            ),
                            const SizedBox(height: 10),
                            HostPanelField(
                              key: const ValueKey('serial-user'),
                              controller: _user,
                              placeholder: AppLanguage.tr(
                                context,
                                'Username',
                              ),
                              leading: LucideIcons.user,
                            ),
                            const SizedBox(height: 10),
                            HostPanelField(
                              key: const ValueKey('serial-password'),
                              controller: _password,
                              obscure: true,
                              placeholder: widget.host?.hasPassword == true
                                  ? AppLanguage.tr(
                                      context,
                                      'Password stored — leave blank to keep',
                                    )
                                  : AppLanguage.tr(
                                      context,
                                      'Password (optional)',
                                    ),
                              leading: LucideIcons.keyRound,
                            ),
                          ],
                        ),
                    ],
                  ),
                  if (_error != null || widget.error != null) ...[
                    const SizedBox(height: 10),
                    Text(
                      AppLanguage.tr(context, _error ?? widget.error!),
                      style: theme.typography.normal.copyWith(
                        fontSize: 12,
                        color: XtColors.dangerText(context),
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ),
          // The same band + full-width primary action as the SSH drawer.
          Container(
            color: XtColors.page(context),
            padding: const EdgeInsets.symmetric(horizontal: 15, vertical: 20),
            child: XtActionButton(
              key: const ValueKey('serial-connect'),
              label: editing
                  ? AppLanguage.tr(context, 'Save & connect')
                  : (widget.saveMode
                        ? AppLanguage.tr(context, 'Save')
                        : AppLanguage.tr(context, 'Connect')),
              variant: XtActionVariant.primary,
              height: XtMetrics.controlHeight,
              expand: true,
              onPressed: _submit,
            ),
          ),
        ],
      ),
    );
  }

  static String _parityLetter(String parity) => switch (parity) {
    'odd' => 'O',
    'even' => 'E',
    _ => 'N',
  };
}

/// Editable port field with a dropdown of detected devices.
///
/// The text stays editable (a serial server path is not always an enumerated
/// COM/tty device); the chevron lists what the OS reported and fills the field
/// on selection.
class _PortComboField extends StatefulWidget {
  final TextEditingController controller;
  final List<SerialPortInfo> ports;
  final bool loading;
  final VoidCallback onRefresh;

  const _PortComboField({
    required this.controller,
    required this.ports,
    required this.loading,
    required this.onRefresh,
  });

  @override
  State<_PortComboField> createState() => _PortComboFieldState();
}

class _PortComboFieldState extends State<_PortComboField> {
  final OverlayPortalController _menu = OverlayPortalController();
  final LayerLink _link = LayerLink();
  BuildContext? _anchorContext;
  double _width = 0;

  void _toggle() {
    if (_menu.isShowing) {
      _menu.hide();
    } else {
      _menu.show();
    }
  }

  void _pick(SerialPortInfo port) {
    widget.controller.text = port.name;
    widget.controller.selection = TextSelection.collapsed(
      offset: port.name.length,
    );
    _menu.hide();
  }

  @override
  Widget build(BuildContext context) {
    return OverlayPortal(
      controller: _menu,
      overlayChildBuilder: _overlay,
      child: CompositedTransformTarget(
        link: _link,
        child: LayoutBuilder(
          builder: (context, constraints) {
            _anchorContext = context;
            if (constraints.maxWidth.isFinite) {
              _width = constraints.maxWidth;
            }
            return XtField(
              key: const ValueKey('serial-port'),
              controller: widget.controller,
              placeholder: AppLanguage.tr(
                context,
                'COM3 or /dev/ttyUSB0',
              ),
              leading: Icon(
                LucideIcons.ethernetPort,
                size: 14,
                color: XtColors.textTertiary(context),
              ),
              trailing: GestureDetector(
                key: const ValueKey('serial-port-toggle'),
                behavior: HitTestBehavior.opaque,
                onTap: _toggle,
                child: SizedBox(
                  width: 22,
                  height: 22,
                  child: Center(
                    child: widget.loading
                        ? const SizedBox(
                            width: 12,
                            height: 12,
                            child: CircularProgressIndicator(strokeWidth: 1.5),
                          )
                        : Icon(
                            _menu.isShowing
                                ? LucideIcons.chevronUp
                                : LucideIcons.chevronDown,
                            size: 15,
                            color: XtColors.textTertiary(context),
                          ),
                  ),
                ),
              ),
            );
          },
        ),
      ),
    );
  }

  Widget _overlay(BuildContext context) {
    final screen = MediaQuery.sizeOf(context);
    final box = _anchorContext?.findRenderObject() as RenderBox?;
    var maxHeight = 260.0;
    if (box != null && box.hasSize) {
      final origin = box.localToGlobal(Offset.zero);
      final below = screen.height - (origin.dy + box.size.height + 6);
      maxHeight = (below - 10).clamp(100.0, 260.0);
    }
    return Stack(
      children: [
        Positioned.fill(
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: _menu.hide,
            child: const ColoredBox(color: Colors.transparent),
          ),
        ),
        CompositedTransformFollower(
          link: _link,
          targetAnchor: Alignment.bottomLeft,
          followerAnchor: Alignment.topLeft,
          offset: const Offset(0, 6),
          showWhenUnlinked: false,
          child: Align(
            alignment: Alignment.topLeft,
            child: XtMenuPopup(
              width: _width > 0 ? _width : 320,
              maxHeight: maxHeight,
              children: [
                if (widget.ports.isEmpty)
                  Padding(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 9,
                      vertical: 10,
                    ),
                    child: Text(
                      widget.loading
                          ? AppLanguage.tr(
                              context,
                              'Scanning serial ports…',
                            )
                          : AppLanguage.tr(
                              context,
                              'No serial ports detected — type a device path',
                            ),
                      style: TextStyle(
                        fontSize: 12,
                        color: XtColors.textTertiary(context),
                      ),
                    ),
                  )
                else
                  for (final port in widget.ports)
                    XtMenuEntry(
                      key: ValueKey('serial-port-${port.name}'),
                      label: port.name,
                      selected: widget.controller.text == port.name,
                      trailing: Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          if (widget.controller.text == port.name) ...[
                            Icon(
                              LucideIcons.check,
                              size: 13,
                              color: XtColors.accent(context),
                            ),
                            const SizedBox(width: 6),
                          ],
                          Text(
                            port.kind,
                            style: TextStyle(
                              fontSize: 11,
                              color: XtColors.textTertiary(context),
                            ),
                          ),
                        ],
                      ),
                      onTap: () => _pick(port),
                    ),
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: 4),
                  child: const XtDivider(),
                ),
                XtMenuEntry(
                  key: const ValueKey('serial-port-refresh'),
                  icon: LucideIcons.refreshCw,
                  label: AppLanguage.tr(context, 'Refresh ports'),
                  onTap: () {
                    _menu.hide();
                    widget.onRefresh();
                  },
                ),
              ],
            ),
          ),
        ),
      ],
    );
  }
}
