import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/data/hosts.dart';
import 'package:xenterm_app/screens/hosts/host_details_panel.dart';
import 'package:xenterm_app/theme/app_theme.dart';
import 'package:xenterm_app/widgets/primitives.dart';

void main() {
  test('legacy MAC defaults off and explicit core consent survives copying', () {
    expect(const Host(id: 'old', label: 'old', address: 'localhost')
        .allowLegacyHmacMd5, isFalse);
    expect(Host.fromCoreJson({'id': 'old'}).allowLegacyHmacMd5, isFalse);
    final enabled = Host.fromCoreJson({
      'id': 'legacy', 'allow_legacy_hmac_md5': true,
    });
    expect(enabled.allowLegacyHmacMd5, isTrue);
    expect(enabled.copyWith(os: HostOs.linux).allowLegacyHmacMd5, isTrue);
  });

  testWidgets('legacy MAC consent is an explicit drawer edit', (tester) async {
    Host? saved;
    await tester.pumpWidget(ShadcnApp(
      theme: ThemeData(colorScheme: xtDarkScheme()),
      home: Scaffold(child: SizedBox(width: 700, height: 900,
        child: HostDetailsPanel(
          host: const Host(id: 'legacy', label: 'legacy', address: '127.0.0.1'),
          onClose: () {}, onConnect: () {},
          onDelete: (_) async => null,
          onSave: (host, password, clearPassword, keyInline, clearKey) async {
            saved = host;
            return null;
          },
        ),
      )),
    ));
    await tester.pumpAndSettle();
    final toggle = find.byKey(const ValueKey('allow-legacy-hmac-md5'));
    expect(tester.widget<XtSwitch>(toggle).value, isFalse);
    await tester.ensureVisible(toggle);
    await tester.tap(toggle);
    await tester.pumpAndSettle();
    expect(tester.widget<XtSwitch>(toggle).value, isTrue);
    final save = find.byWidgetPredicate(
      (widget) => widget is XtIconButton && widget.icon == LucideIcons.check,
    );
    await tester.ensureVisible(save);
    await tester.tap(save);
    await tester.pumpAndSettle();
    expect(saved?.allowLegacyHmacMd5, isTrue);
  });
}
