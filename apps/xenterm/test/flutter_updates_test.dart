import 'dart:convert';
import 'dart:typed_data';

import 'package:crypto/crypto.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:xenterm_app/data/flutter_updates.dart';

const _repo = 'xue-ding-e/XenTerm-Next';
const _github = 'https://github.com/$_repo/releases';
const _running = 'xt-beta-f48f7e7-T1793094015';
const _flutterSha = '5d50ca8bf279e173b616c9847199cb15be36473a';
const _flutterVersion = 'xt-beta-5d50ca8-T1794599048';

Uint8List _bytes(Object value) =>
    Uint8List.fromList(utf8.encode(jsonEncode(value)));

({
  Map<String, dynamic> release,
  Map<String, Uint8List> metadata,
  String packageName,
})
_flutterRelease({
  String sha = _flutterSha,
  String version = _flutterVersion,
  String platform = 'windows-x86_64',
  String product = 'XenTerm Next Flutter',
  bool promoted = true,
}) {
  final tag = 'flutter-beta-${sha.substring(0, 12)}';
  final stem = 'XenTerm-Next-Flutter-$sha-$platform';
  final manifest = '$stem-BUILD.json';
  final package = '$stem.zip';
  final checksum = '$stem.sha256';
  final base = '$_github/download/$tag';
  final content = utf8.encode('flutter payload');
  final digest = sha256.convert(content).toString();
  final sums = Uint8List.fromList(utf8.encode('$digest  $package\n'));
  final receipt = _bytes({
    'schema_version': 1,
    'product': product,
    'frontend': 'flutter',
    'channel': 'flutter-beta',
    'repository': _repo,
    'source_sha': sha,
    'platform': platform,
    'runtime_version': version,
    'files': [
      {'name': package, 'size': content.length, 'sha256': digest},
    ],
  });
  Map<String, dynamic> asset(String name, List<int> payload) => {
    'name': name,
    'browser_download_url': '$base/$name',
    'state': 'uploaded',
    'digest': 'sha256:${sha256.convert(payload)}',
    'size': payload.length,
  };
  return (
    release: {
      'tag_name': tag,
      'html_url': '$_github/tag/$tag',
      'target_commitish': sha,
      'prerelease': true,
      'draft': false,
      'body': promoted
          ? '**$platform** — `$version`\n- [BUILD.json]($base/$manifest)'
          : '',
      'assets': [
        asset(package, content),
        asset(manifest, receipt),
        asset(checksum, sums),
      ],
    },
    metadata: {'$base/$manifest': receipt, '$base/$checksum': sums},
    packageName: package,
  );
}

FlutterUpdateSource _source(
  List<Map<String, dynamic>> releases,
  Map<String, Uint8List> assets,
) => FlutterUpdateSource(
  read: (uri) async {
    if (uri.host == 'api.github.com') return _bytes(releases);
    final asset = assets[uri.toString()];
    if (asset == null) fail('Unexpected asset query $uri');
    return asset;
  },
);

void main() {
  test('only offers Flutter when GPUI and Flutter releases coexist', () async {
    final flutter = _flutterRelease();
    final gpui = {
      ...flutter.release,
      'tag_name': 'beta',
      'html_url': '$_github/tag/beta',
    };
    final update = await _source([
      gpui,
      flutter.release,
    ], flutter.metadata).check(_running, FlutterUpdateTarget.windowsZip);
    expect(update?.version, _flutterVersion);
    expect(
      await _source([gpui], {}).check(_running, FlutterUpdateTarget.windowsZip),
      isNull,
    );
  });

  test('requires the running platform to have a promoted package', () async {
    final ready = _flutterRelease();
    final pending = _flutterRelease(
      sha: 'abaf9c523780a608bd46686fd5e53740a07077f8',
      version: 'xt-beta-abaf9c5-T1794599050',
      promoted: false,
    );
    final onlyMac = _flutterRelease(
      sha: '778441ee1253ca46c1cd539d295b59c3c50ba9eb',
      version: 'xt-beta-778441e-T1794599051',
      platform: 'macos-aarch64',
    );
    final source = _source([
      pending.release,
      onlyMac.release,
      ready.release,
    ], ready.metadata);
    expect(
      (await source.check(_running, FlutterUpdateTarget.windowsZip))?.version,
      _flutterVersion,
    );
    expect(await source.check(_running, FlutterUpdateTarget.linux), isNull);
  });

  test('ignores same source and older builds', () async {
    final flutter = _flutterRelease();
    final source = _source([flutter.release], {});
    expect(
      await source.check(
        'xt-beta-5d50ca8-T1793094015',
        FlutterUpdateTarget.windowsZip,
      ),
      isNull,
    );
    expect(
      await source.check(
        'xt-beta-f48f7e7-T1794599999',
        FlutterUpdateTarget.windowsZip,
      ),
      isNull,
    );
  });

  test(
    'rejects another product even when its receipt is served on a Flutter tag',
    () async {
      final flutter = _flutterRelease(product: 'XenTerm Next GPUI');
      await expectLater(
        _source([
          flutter.release,
        ], flutter.metadata).check(_running, FlutterUpdateTarget.windowsZip),
        throwsFormatException,
      );
    },
  );

  test(
    'rejects a mismatched checksum and a missing Windows installer',
    () async {
      final flutter = _flutterRelease();
      final modified = {...flutter.metadata};
      modified[modified.keys.last] = Uint8List.fromList(
        utf8.encode('damaged manifest'),
      );
      await expectLater(
        _source([
          flutter.release,
        ], modified).check(_running, FlutterUpdateTarget.windowsZip),
        throwsFormatException,
      );
      await expectLater(
        _source([
          flutter.release,
        ], flutter.metadata).check(_running, FlutterUpdateTarget.windowsMsi),
        throwsFormatException,
      );
    },
  );
}
