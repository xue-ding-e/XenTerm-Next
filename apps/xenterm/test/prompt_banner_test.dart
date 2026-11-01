import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/screens/terminal/connection_prompt.dart';
import 'package:xenterm_app/theme/app_theme.dart';

Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: SizedBox(width: 800, height: 600, child: child)),
);

void main() {
  testWidgets('host-key prompt shows the fingerprint and trusts on demand', (
    tester,
  ) async {
    bool? accepted;
    await tester.pumpWidget(
      _app(
        ConnectionPromptBanner(
          prompt: const {
            'kind': 'host_key',
            'host': '10.0.0.2',
            'port': 22,
            'key_type': 'ssh-ed25519',
            'fingerprint': 'SHA256:abc',
            'changed': false,
          },
          onAnswer: (value, {username, password, text}) => accepted = value,
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(
      find.textContaining('Unknown host key for 10.0.0.2:22'),
      findsOneWidget,
    );
    expect(find.textContaining('SHA256:abc'), findsOneWidget);
    await tester.tap(find.text('Trust'));
    await tester.pumpAndSettle();
    expect(accepted, isTrue);
  });

  testWidgets('credential prompt forwards the typed password', (tester) async {
    String? captured;
    await tester.pumpWidget(
      _app(
        ConnectionPromptBanner(
          prompt: const {
            'kind': 'credential',
            'host': '10.0.0.2',
            'user': 'demo',
            'need_user': false,
            'need_password': true,
          },
          onAnswer: (value, {username, password, text}) {
            captured = password;
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(EditableText).last, 'secret');
    await tester.tap(find.text('Continue'));
    await tester.pumpAndSettle();
    expect(captured, 'secret');
  });
}
