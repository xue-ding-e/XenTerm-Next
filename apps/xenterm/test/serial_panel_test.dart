import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';
import 'package:xenterm_app/data/hosts.dart';
import 'package:xenterm_app/screens/hosts/serial_details_panel.dart';
import 'package:xenterm_app/theme/app_field.dart';
import 'package:xenterm_app/theme/app_theme.dart';

Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: SizedBox(width: 360, height: 900, child: child)),
);

Future<void> _surface(WidgetTester tester) async {
  await tester.binding.setSurfaceSize(const Size(800, 1200));
  addTearDown(() => tester.binding.setSurfaceSize(null));
}

void main() {
  testWidgets('the serial panel collects port, line settings and login', (
    tester,
  ) async {
    SerialDraft? submitted;
    await _surface(tester);
    await tester.pumpWidget(
      _app(
        SerialDetailsPanel(
          loadPorts: () async => const [
            SerialPortInfo(name: 'COM3', kind: 'USB · FTDI'),
            SerialPortInfo(name: 'COM7', kind: 'Serial'),
          ],
          onSubmit: (draft) => submitted = draft,
          onClose: () {},
        ),
      ),
    );
    await tester.pumpAndSettle();

    // The first attached port is selected for a new connection.
    expect(
      tester
          .widget<XtField>(find.byKey(const ValueKey('serial-port')))
          .controller
          ?.text,
      'COM3',
    );

    // The port field is a dropdown: open it and pick another detected device.
    await tester.tap(find.byKey(const ValueKey('serial-port-toggle')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('serial-port-COM3')), findsOne);
    expect(find.byKey(const ValueKey('serial-port-COM7')), findsOne);
    await tester.tap(find.byKey(const ValueKey('serial-port-COM7')));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<XtField>(find.byKey(const ValueKey('serial-port')))
          .controller
          ?.text,
      'COM7',
    );

    await tester.enterText(
      find.byKey(const ValueKey('serial-label')),
      'Console',
    );

    await tester.tap(find.byKey(const ValueKey('serial-advanced')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('serial-user')), findsOne);

    await tester.tap(find.byKey(const ValueKey('serial-parity')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Even').last);
    await tester.pumpAndSettle();
    // The select popup covers the panel after a pick; close it first.
    await tester.tapAt(const Offset(4, 4));
    await tester.pumpAndSettle();

    await tester.enterText(find.byKey(const ValueKey('serial-user')), 'admin');
    await tester.enterText(
      find.byKey(const ValueKey('serial-password')),
      'console-pass',
    );
    await tester.tap(find.byKey(const ValueKey('serial-connect')));
    await tester.pumpAndSettle();

    expect(submitted, isNotNull);
    expect(submitted!.label, 'Console');
    expect(submitted!.port, 'COM7');
    expect(submitted!.baudRate, 115200);
    expect(submitted!.parity, 'even');
    expect(submitted!.username, 'admin');
    expect(submitted!.password, 'console-pass');
  });

  testWidgets('editing a serial host prefills its frame and keeps the secret', (
    tester,
  ) async {
    SerialDraft? submitted;
    await _surface(tester);
    await tester.pumpWidget(
      _app(
        SerialDetailsPanel(
          host: const Host(
            id: 'serial-1',
            label: 'Switch console',
            address: 'COM4',
            username: 'ops',
            protocol: 'serial',
            baudRate: 9600,
            dataBits: 7,
            stopBits: 2,
            parity: 'odd',
            flowControl: 'hardware',
            hasPassword: true,
          ),
          loadPorts: () async => const [],
          onSubmit: (draft) => submitted = draft,
          onClose: () {},
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Serial Details'), findsOne);
    expect(find.text('9600'), findsOne);
    await tester.tap(find.byKey(const ValueKey('serial-advanced')));
    await tester.pumpAndSettle();
    expect(find.text('Password stored — leave blank to keep'), findsOne);

    await tester.tap(find.byKey(const ValueKey('serial-connect')));
    await tester.pumpAndSettle();
    expect(submitted!.port, 'COM4');
    expect(submitted!.baudRate, 9600);
    expect(submitted!.dataBits, 7);
    expect(submitted!.stopBits, 2);
    expect(submitted!.parity, 'odd');
    expect(submitted!.flowControl, 'hardware');
    expect(submitted!.password, '');
  });

  testWidgets('a device path can be typed without picking from the list', (
    tester,
  ) async {
    SerialDraft? submitted;
    await _surface(tester);
    await tester.pumpWidget(
      _app(
        SerialDetailsPanel(
          loadPorts: () async => const [
            SerialPortInfo(name: 'COM3', kind: 'USB'),
          ],
          onSubmit: (draft) => submitted = draft,
          onClose: () {},
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.enterText(
      find.byKey(const ValueKey('serial-port')),
      r'\\.\COM42',
    );
    await tester.tap(find.byKey(const ValueKey('serial-connect')));
    await tester.pumpAndSettle();
    expect(submitted!.port, r'\\.\COM42');
  });
}
