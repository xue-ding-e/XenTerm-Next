/// The published Next Flutter desktop beta, including per-platform receipts.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:typed_data';

import 'package:crypto/crypto.dart';
import 'package:ffi/ffi.dart';
import 'package:xml/xml.dart';

const _repository = 'xue-ding-e/XenTerm-Next';
const _releaseBase = 'https://github.com/$_repository/releases';
const _listApi = 'https://api.github.com/repos/$_repository/releases';
final _flutterTag = RegExp(r'^flutter-beta-[a-f0-9]{12}$');
final _fullSha = RegExp(r'^[a-f0-9]{40}$');
final _sha256 = RegExp(r'^[a-f0-9]{64}$');
final _betaVersion = RegExp(r'^xt-beta-([a-f0-9]{7})-T([1-9][0-9]{9})$');
const _maxMetadata = 4 * 1024 * 1024;
const _maxPackage = 512 * 1024 * 1024;

enum FlutterUpdateTarget {
  windowsZip('windows-x86_64', 'zip'),
  windowsMsi('windows-x86_64', 'msi'),
  linux('linux-x86_64', 'tar.gz'),
  macos('macos-aarch64', 'zip');

  const FlutterUpdateTarget(this.platform, this.extension);

  final String platform;
  final String extension;

  static FlutterUpdateTarget? current() => switch (Abi.current()) {
    Abi.windowsX64 =>
      _isMsiInstallation()
          ? FlutterUpdateTarget.windowsMsi
          : FlutterUpdateTarget.windowsZip,
    Abi.linuxX64 => FlutterUpdateTarget.linux,
    Abi.macosArm64 => FlutterUpdateTarget.macos,
    _ => null,
  };
}

typedef MetadataReader = Future<Uint8List> Function(Uri uri);

class FlutterUpdateSource {
  FlutterUpdateSource({MetadataReader? read}) : _read = read ?? _readMetadata;

  final MetadataReader _read;

  static bool supports(String? version, FlutterUpdateTarget? target) =>
      target != null && version != null && _betaVersion.hasMatch(version);

  Future<FlutterUpdate?> check(
    String version,
    FlutterUpdateTarget target,
  ) async {
    final running = _betaVersion.firstMatch(version);
    if (running == null) return null;
    List<_Candidate> candidates;
    try {
      candidates = await _apiCandidates(target);
    } on _ReleaseRateLimited {
      candidates = await _feedCandidates(target);
    }
    final timestamp = int.parse(running.group(2)!);
    candidates.removeWhere(
      (candidate) =>
          candidate.hash == running.group(1) ||
          candidate.timestamp <= timestamp,
    );
    candidates.sort((a, b) => b.timestamp.compareTo(a.timestamp));
    if (candidates.isEmpty) return null;
    var candidate = candidates.first;
    if (candidate.fromFeed) {
      final tag = candidate.tag;
      final html = utf8.decode(
        await _read(Uri.parse('$_releaseBase/expanded_assets/$tag')),
      );
      candidate = candidate.withWebAssets(html);
    }
    return _verify(candidate, target);
  }

  Future<List<_Candidate>> _apiCandidates(FlutterUpdateTarget target) async {
    final candidates = <_Candidate>[];
    const pageSize = 50;
    for (var page = 1; ; page++) {
      final uri = Uri.parse('$_listApi?per_page=$pageSize&page=$page');
      final releases =
          jsonDecode(utf8.decode(await _read(uri))) as List<dynamic>;
      for (final release in releases.whereType<Map<String, dynamic>>()) {
        final candidate = _Candidate.parse(release, target);
        if (candidate != null) candidates.add(candidate);
      }
      if (releases.length < pageSize) break;
    }
    return candidates;
  }

  Future<List<_Candidate>> _feedCandidates(FlutterUpdateTarget target) async {
    final feed = XmlDocument.parse(
      utf8.decode(await _read(Uri.parse('$_releaseBase.atom'))),
    );
    final candidates = <_Candidate>[];
    for (final entry in feed.findAllElements('entry')) {
      final page = entry.getElement('link')?.getAttribute('href') ?? '';
      const tagPrefix = '$_releaseBase/tag/';
      if (!page.startsWith(tagPrefix)) continue;
      final tag = page.substring(tagPrefix.length);
      if (!_flutterTag.hasMatch(tag)) continue;
      final body = entry.getElement('content')?.innerText ?? '';
      final prefix = RegExp.escape(
        '$_releaseBase/download/$tag/XenTerm-Next-Flutter-',
      );
      final platform = RegExp.escape(target.platform);
      final shaPattern = RegExp(
        'href="$prefix([a-f0-9]{40})-$platform-BUILD[.]json"',
      );
      final matches = shaPattern.allMatches(body).toList();
      if (matches.length != 1) continue;
      final commit = matches.single.group(1)!;
      if (!commit.startsWith(tag.substring('flutter-beta-'.length))) continue;
      final headers = RegExp(
        '<strong>$platform</strong> — <code>([^<]+)</code>',
      ).allMatches(body).toList();
      if (headers.length != 1) continue;
      final version = headers.single.group(1)!;
      if (_betaVersion.firstMatch(version)?.group(1) !=
          commit.substring(0, 7)) {
        continue;
      }
      candidates.add(
        _Candidate(
          tag,
          commit,
          version,
          'XenTerm-Next-Flutter-$commit-${target.platform}',
          const [],
          fromFeed: true,
        ),
      );
    }
    return candidates;
  }

  Future<FlutterUpdate> _verify(
    _Candidate candidate,
    FlutterUpdateTarget target,
  ) async {
    final manifest = candidate.asset(
      '${candidate.stem}-BUILD.json',
      _maxMetadata,
    );
    final checksums = candidate.asset('${candidate.stem}.sha256', _maxMetadata);
    final package = candidate.asset(
      '${candidate.stem}.${target.extension}',
      _maxPackage,
    );
    final receipt = jsonDecode(
      utf8.decode(await manifest.verifiedMetadata(_read)),
    ) as Map<String, dynamic>;
    if (receipt['schema_version'] != 1 ||
        receipt['product'] != 'XenTerm Next Flutter' ||
        receipt['repository'] != _repository ||
        receipt['frontend'] != 'flutter' ||
        receipt['channel'] != 'flutter-beta' ||
        receipt['source_sha'] != candidate.commit ||
        receipt['platform'] != target.platform ||
        receipt['runtime_version'] != candidate.version) {
      throw const FormatException('Flutter update receipt identity mismatch');
    }
    final files = (receipt['files'] as List<dynamic>)
        .whereType<Map<String, dynamic>>()
        .where((row) => row['name'] == package.name)
        .toList();
    final sums = const LineSplitter().convert(
      utf8.decode(await checksums.verifiedMetadata(_read)),
    );
    final matching = sums
        .where((line) => line.endsWith('  ${package.name}'))
        .toList();
    final size = files.length == 1 ? files.single['size'] : null;
    if (files.length != 1 ||
        files.single['sha256'] != package.digest ||
        size is! int ||
        size < 1 ||
        size > _maxPackage ||
        (package.size != null && size != package.size) ||
        matching.length != 1 ||
        matching.single != '${package.digest}  ${package.name}') {
      throw const FormatException('Flutter update package checksum mismatch');
    }
    return FlutterUpdate._(
      candidate.version,
      target,
      _Asset(package.name, package.url, package.digest, size),
    );
  }
}

class FlutterUpdate {
  const FlutterUpdate._(this.version, this.target, this._asset);

  final String version;
  final FlutterUpdateTarget target;
  final _Asset _asset;

  Future<File> download(Directory? folder) async {
    final directory = await (folder ?? Directory.systemTemp).createTemp(
      'xenterm-next-flutter-',
    );
    try {
      final path = '${directory.path}${Platform.pathSeparator}${_asset.name}';
      final partial = File('$path.part');
      await _withResponse(_asset.url, (response) async {
        final output = await partial.open(mode: FileMode.write);
        try {
          final digest = await sha256
              .bind(
                _asset
                    .verifiedLength(
                      response.timeout(const Duration(seconds: 30)),
                    )
                    .asyncMap((chunk) async {
                      await output.writeFrom(chunk);
                      return chunk;
                    }),
              )
              .single;
          if (digest.toString() != _asset.digest) {
            throw const FormatException(
              'Flutter update download failed SHA-256 verification',
            );
          }
          await output.flush();
        } finally {
          await output.close();
        }
      }, timeout: const Duration(minutes: 5));
      return await partial.rename(path);
    } catch (_) {
      // This directory was created exclusively for this download.
      await directory.delete(recursive: true);
      rethrow;
    }
  }

  Future<void> install(File package) async {
    if (target != FlutterUpdateTarget.windowsMsi || !_isMsiInstallation()) {
      throw StateError(
        'This executable is outside the Flutter MSI installation',
      );
    }
    await Process.start(_systemTool('msiexec.exe'), [
      '/i',
      package.path,
      '/norestart',
    ], mode: ProcessStartMode.detached);
  }
}

class _Candidate {
  _Candidate(
    this.tag,
    this.commit,
    this.version,
    this.stem,
    this.assets, {
    this.fromFeed = false,
  });

  final String tag;
  final String commit;
  final String version;
  final String stem;
  final List<dynamic> assets;
  final bool fromFeed;

  String get hash => version.substring(8, 15);
  int get timestamp => int.parse(version.substring(17));

  _Candidate withWebAssets(String html) {
    final pattern = RegExp(
      r'<clipboard-copy [^>]*aria-label="Copy to clipboard digest for ([A-Za-z0-9._-]+)"[^>]*value="(sha256:[a-f0-9]{64})"',
    );
    final found = [
      for (final match in pattern.allMatches(html))
        <String, dynamic>{
          'name': match.group(1)!,
          'digest': match.group(2)!,
          'state': 'uploaded',
          'browser_download_url':
              '$_releaseBase/download/$tag/${match.group(1)}',
        },
    ];
    return _Candidate(tag, commit, version, stem, found, fromFeed: true);
  }

  static _Candidate? parse(
    Map<String, dynamic> release,
    FlutterUpdateTarget target,
  ) {
    final tag = release['tag_name'];
    final commit = release['target_commitish'];
    final body = release['body'];
    final assets = release['assets'];
    if (tag is! String ||
        !_flutterTag.hasMatch(tag) ||
        commit is! String ||
        !_fullSha.hasMatch(commit) ||
        tag != 'flutter-beta-${commit.substring(0, 12)}' ||
        release['draft'] != false ||
        release['prerelease'] != true ||
        release['html_url'] != '$_releaseBase/tag/$tag' ||
        body is! String ||
        assets is! List) {
      return null;
    }
    final stem = 'XenTerm-Next-Flutter-$commit-${target.platform}';
    // Platform assets can be uploading while the same release serves another
    // OS. The publisher only advertises a platform once its assets are verified.
    final prefix = '**${target.platform}** — `';
    final lines = const LineSplitter().convert(body);
    final headers = lines
        .where((line) => line.startsWith(prefix) && line.endsWith('`'))
        .toList();
    final manifestUrl = '$_releaseBase/download/$tag/$stem-BUILD.json';
    if (headers.length != 1 ||
        !lines.contains('- [BUILD.json]($manifestUrl)')) {
      return null;
    }
    final version = headers.single.substring(
      prefix.length,
      headers.single.length - 1,
    );
    final parsed = _betaVersion.firstMatch(version);
    if (parsed == null || parsed.group(1) != commit.substring(0, 7)) {
      return null;
    }
    return _Candidate(tag, commit, version, stem, assets);
  }

  _Asset asset(String name, int maxSize) {
    final matches = assets.whereType<Map<String, dynamic>>().where(
      (asset) => asset['name'] == name,
    );
    if (matches.length != 1) {
      throw FormatException('Missing or ambiguous Flutter asset $name');
    }
    final item = matches.single;
    final url = '$_releaseBase/download/$tag/$name';
    final digest = item['digest'];
    final size = item['size'];
    if (item['state'] != 'uploaded' ||
        item['browser_download_url'] != url ||
        (!(fromFeed && size == null) &&
            (size is! int || size < 1 || size > maxSize)) ||
        digest is! String ||
        !digest.startsWith('sha256:') ||
        !_sha256.hasMatch(digest.substring(7))) {
      throw FormatException('Invalid Flutter asset $name');
    }
    return _Asset(name, Uri.parse(url), digest.substring(7), size as int?);
  }
}

class _Asset {
  const _Asset(this.name, this.url, this.digest, this.size);

  final String name;
  final Uri url;
  final String digest;
  final int? size;

  Future<Uint8List> verifiedMetadata(MetadataReader read) async {
    final bytes = await read(url);
    if (bytes.isEmpty ||
        (size != null && bytes.length != size) ||
        sha256.convert(bytes).toString() != digest) {
      throw FormatException('Flutter asset checksum mismatch for $name');
    }
    return bytes;
  }

  Stream<List<int>> verifiedLength(Stream<List<int>> input) async* {
    final expected = size!;
    var received = 0;
    await for (final chunk in input) {
      received += chunk.length;
      if (received > expected) {
        throw const FormatException('Oversized Flutter update asset');
      }
      yield chunk;
    }
    if (received != expected) {
      throw const FormatException('Incomplete Flutter update asset');
    }
  }
}

class _ReleaseRateLimited implements Exception {}

Future<T> _withResponse<T>(
  Uri uri,
  Future<T> Function(HttpClientResponse response) read, {
  required Duration timeout,
}) async {
  final client = HttpClient()
    ..connectionTimeout = const Duration(seconds: 8)
    ..userAgent = 'XenTerm-Next-Flutter'
    ..findProxy = _proxyFinder();
  try {
    final request = await client
        .getUrl(uri)
        .timeout(const Duration(seconds: 15));
    request.headers.set(
      HttpHeaders.acceptHeader,
      'application/vnd.github+json',
    );
    final response = await request.close().timeout(const Duration(seconds: 15));
    if (response.statusCode == 429 ||
        (response.statusCode == HttpStatus.forbidden &&
            response.headers.value('x-ratelimit-remaining') == '0')) {
      throw _ReleaseRateLimited();
    }
    if (response.statusCode != HttpStatus.ok) {
      throw HttpException(
        'Flutter release returned ${response.statusCode}',
        uri: uri,
      );
    }
    final deadline = Timer(timeout, () => client.close(force: true));
    try {
      return await read(response);
    } finally {
      deadline.cancel();
    }
  } finally {
    client.close(force: true);
  }
}

Future<Uint8List> _readMetadata(Uri uri) =>
    _withResponse(uri, (response) async {
      final bytes = BytesBuilder(copy: false);
      await for (final chunk in response) {
        if (bytes.length + chunk.length > _maxMetadata) {
          throw const FormatException('Oversized Flutter release metadata');
        }
        bytes.add(chunk);
      }
      return bytes.takeBytes();
    }, timeout: const Duration(seconds: 15));

typedef _RegGetValue = Int32 Function(
  IntPtr,
  Pointer<Utf16>,
  Pointer<Utf16>,
  Uint32,
  Pointer<Uint32>,
  Pointer<Void>,
  Pointer<Uint32>,
);

T? _registryValue<T>(
  String key,
  String name,
  int flags,
  T Function(Pointer<Uint16>) decode,
) => using((arena) {
  final registry = DynamicLibrary.open('advapi32.dll')
      .lookupFunction<
        _RegGetValue,
        int Function(
          int,
          Pointer<Utf16>,
          Pointer<Utf16>,
          int,
          Pointer<Uint32>,
          Pointer<Void>,
          Pointer<Uint32>,
        )
      >('RegGetValueW');
  final buffer = arena<Uint16>(4096);
  final bytes = arena<Uint32>()..value = 8192;
  final result = registry(
    -2147483647, // HKCU
    key.toNativeUtf16(allocator: arena),
    name.toNativeUtf16(allocator: arena),
    flags,
    nullptr,
    buffer.cast(),
    bytes,
  );
  return result == 0 ? decode(buffer) : null;
});

String? _registryText(String key, String name) => _registryValue(
  key,
  name,
  0x10002,
  (buffer) => buffer.cast<Utf16>().toDartString(),
);

String Function(Uri) _proxyFinder() {
  final environment = Platform.environment;
  if (!Platform.isWindows ||
      (environment['https_proxy'] ?? environment['HTTPS_PROXY'])?.isNotEmpty ==
          true) {
    return HttpClient.findProxyFromEnvironment;
  }
  const key = r'Software\Microsoft\Windows\CurrentVersion\Internet Settings';
  final enabled = _registryValue(
    key,
    'ProxyEnable',
    0x10,
    (buffer) => buffer.cast<Uint32>().value,
  );
  final server = enabled == 1 ? _registryText(key, 'ProxyServer') : null;
  if (server == null || server.isEmpty) {
    return HttpClient.findProxyFromEnvironment;
  }
  final routes = server.split(';').map((value) => value.trim()).toList();
  final https = routes.where(
    (route) => route.toLowerCase().startsWith('https='),
  );
  final proxy = server.contains('=')
      ? https.firstOrNull?.substring(6).trim()
      : routes.first;
  if (proxy == null || proxy.isEmpty) {
    return HttpClient.findProxyFromEnvironment;
  }

  final bypass = (_registryText(key, 'ProxyOverride') ?? '').split(';').map((
    rule,
  ) {
    final pattern = rule.trim().split('*').map(RegExp.escape).join('.*');
    return RegExp(r'^' + pattern + r'$', caseSensitive: false);
  }).toList();
  return (uri) {
    if (bypass.any((rule) => rule.hasMatch(uri.host))) return 'DIRECT';
    return HttpClient.findProxyFromEnvironment(
      uri,
      environment: {
        'https_proxy': proxy,
        'no_proxy': environment['no_proxy'] ?? environment['NO_PROXY'] ?? '',
      },
    );
  };
}

bool _isMsiInstallation() {
  if (!Platform.isWindows) return false;
  try {
    final folder = _registryText(
      r'Software\XenTerm-Next\Flutter',
      'InstallLocation',
    );
    return folder != null &&
        folder.isNotEmpty &&
        FileSystemEntity.identicalSync(
          Platform.resolvedExecutable,
          '$folder\\xenterm.exe',
        );
  } on Object {
    return false;
  }
}

String _systemTool(String name) {
  final root = Platform.environment['SystemRoot'];
  if (root == null || root.isEmpty) {
    throw StateError('Windows system directory is unavailable');
  }
  return '$root\\System32\\$name';
}
