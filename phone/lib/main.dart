/// Noctorium's installer for a phone.
///
/// One screen and a button for each thing it installs: Noctorium, and Noctorium Stats beside it. It asks
/// GitHub for the latest release, downloads the APK, checks it against the checksum published beside it,
/// and hands it to Android's own package installer -- which then shows its own screen and asks again, and
/// the first time also sends you to a settings page to allow this application to install others at all.
/// Nothing here installs anything itself, and that is deliberate: three steps between a download and a
/// new application, all three belonging to Android.
library;

import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:http/http.dart' as http;
import 'package:path_provider/path_provider.dart';

import 'release.dart';

/// Where releases are published. The same default the desktop installer uses.
const String repository = String.fromEnvironment(
  'NOCTORIUM_REPOSITORY',
  defaultValue: 'Noctorium/Noctorium-Installer',
);

/// Handed to the Kotlin side, which fires the intent. Android will not let Dart do that itself.
const MethodChannel installerChannel = MethodChannel('app.noctorium.installer/install');

void main() => runApp(const InstallerApp());

/// How the latest release is found out about: GitHub, unless a test says otherwise.
typedef ReleaseLookup = Future<Release> Function();

/// What this installs.
enum Product {
  noctorium('Noctorium'),
  stats('Noctorium Stats');

  const Product(this.title);

  final String title;

  /// Its APK in [release], or null when the release carries none.
  Asset? apkIn(Release release) => switch (this) {
        Product.noctorium => release.apk,
        Product.stats => release.statsApk,
      };
}

class InstallerApp extends StatelessWidget {
  const InstallerApp({super.key, this.lookUp});

  final ReleaseLookup? lookUp;

  @override
  Widget build(BuildContext context) {
    // Noctorium's own colours rather than the system's, so the installer and what it installs look like
    // one thing.
    final scheme = ColorScheme.fromSeed(
      seedColor: const Color(0xFFB47CFF),
      brightness: Brightness.dark,
    ).copyWith(surface: const Color(0xFF08070C));
    return MaterialApp(
      title: 'Noctorium Installer',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(colorScheme: scheme, useMaterial3: true),
      home: InstallerScreen(lookUp: lookUp),
    );
  }
}

/// How far along the one job this application has is.
///
/// [unavailable] is not a failure: Noctorium Stats was asked for of a release from before it, which is
/// said plainly, and Noctorium is still there to install.
enum Stage { idle, asking, downloading, checking, handingOver, done, failed, unavailable }

class InstallerScreen extends StatefulWidget {
  const InstallerScreen({super.key, this.lookUp});

  final ReleaseLookup? lookUp;

  @override
  State<InstallerScreen> createState() => _InstallerScreenState();
}

class _InstallerScreenState extends State<InstallerScreen> {
  Stage _stage = Stage.idle;
  String _message = 'Install the latest Noctorium on this phone, or Noctorium Stats, which shows what '
      'you have listened to.';
  double? _progress;
  String? _version;

  /// The product the last thing that happened was about, whose button says how it went.
  Product _product = Product.noctorium;

  /// The latest release, as far as it was asked about when the screen opened: what says whether there is
  /// a Noctorium Stats to offer. Null until it answers, and if it never does, which only means the button
  /// finds out for itself.
  Release? _latest;

  bool get _busy =>
      _stage == Stage.asking || _stage == Stage.downloading || _stage == Stage.checking || _stage == Stage.handingOver;

  /// Whether Noctorium Stats can be installed from the latest release: false only when it was asked, and
  /// said no.
  bool get _statsThere => _latest == null || _latest!.statsApk != null;

  @override
  void initState() {
    super.initState();
    _lookAhead();
  }

  /// Asks what the latest release carries before anything is pressed, so a button for something it does
  /// not carry is not offered. Asking changes nothing, and a failure here is no failure at all: the
  /// install asks again, and says what went wrong then.
  Future<void> _lookAhead() async {
    try {
      final release = await _release();
      if (mounted) setState(() => _latest = release);
    } catch (_) {
      // Said, if it matters, when somebody presses a button.
    }
  }

  Future<Release> _release() => widget.lookUp?.call() ?? _latestRelease();

  Future<void> _install(Product product) async {
    setState(() {
      _product = product;
      _stage = Stage.asking;
      _progress = null;
      _message = 'Asking GitHub for the latest release...';
    });

    try {
      final release = await _release();
      _latest = release;
      final apk = product.apkIn(release);
      final checksums = release.checksums;
      if (apk == null && product == Product.stats) {
        // Not a failure: a release from before Stats, which is every release until one carries it.
        setState(() {
          _version = release.tag;
          _stage = Stage.unavailable;
          _message = 'Noctorium Stats is not in ${release.tag} yet, so there is nothing to install for it. '
              'It is new, and the release that first carries it will have it. Noctorium itself is there.';
        });
        return;
      }
      if (apk == null) {
        throw const _Refused('The latest release carries no APK. It may have been published for the desktop only.');
      }
      if (checksums == null) {
        throw const _Refused(
          'The release has no SHA256SUMS.txt, so the download cannot be checked against anything. '
          'Refusing to install something unverified.',
        );
      }

      setState(() {
        _version = release.tag;
        _stage = Stage.downloading;
        _message = 'Downloading ${apk.name}';
      });

      final listing = await _text(checksums.url);
      final published = publishedChecksum(listing, apk.name);
      if (published == null) {
        throw _Refused('The release lists no checksum for ${apk.name}, so it cannot be checked.');
      }

      final file = await _download(apk);

      setState(() {
        _stage = Stage.checking;
        _progress = null;
        _message = 'Checking what arrived against the published checksum...';
      });
      final actual = await _sha256(file);
      if (actual != published) {
        // Deleted rather than left about: a file that failed its checksum is the one file nobody should
        // be able to open by accident afterwards.
        await file.delete();
        throw const _Refused(
          'The APK did not match the checksum published with it, so it has not been opened. That is '
          'either a download that went wrong -- try again -- or a file that is not what the release says.',
        );
      }

      setState(() {
        _stage = Stage.handingOver;
        _message = 'Handing it to Android...';
      });
      final complaint = await installerChannel.invokeMethod<String>('install', {'path': file.path});
      if (complaint != null) throw _Refused(complaint);

      setState(() {
        _stage = Stage.done;
        _message = 'Android has ${product.title}. Its own installer takes it from here.\n\n'
            'The first time, it will send you to a settings page to allow this app to install others.';
      });
    } on _Refused catch (refusal) {
      setState(() {
        _stage = Stage.failed;
        _progress = null;
        _message = refusal.why;
      });
    } catch (error) {
      setState(() {
        _stage = Stage.failed;
        _progress = null;
        _message = 'Something went wrong: $error';
      });
    }
  }

  Future<Release> _latestRelease() async {
    final response = await _get(Uri.parse('https://api.github.com/repos/$repository/releases/latest'));
    if (response.statusCode == 404) {
      throw const _Refused(
        'GitHub has no published release for this repository. If it is private, this app cannot see it '
        'either -- download the APK from the releases page yourself.',
      );
    }
    if (response.statusCode != 200) {
      throw _Refused('GitHub refused the request (HTTP ${response.statusCode}).');
    }
    try {
      return Release.fromJson(response.body);
    } on FormatException catch (e) {
      throw _Refused("GitHub's answer could not be read: ${e.message}");
    }
  }

  Future<String> _text(String url) async {
    final response = await _get(Uri.parse(url));
    if (response.statusCode != 200) {
      throw _Refused('Could not read the checksums (HTTP ${response.statusCode}).');
    }
    return response.body;
  }

  Future<http.Response> _get(Uri url) {
    // GitHub refuses a request with no user agent. A token is only ever read from the build, never
    // stored, and is how somebody reaches a private repository.
    const token = String.fromEnvironment('GITHUB_TOKEN');
    return http.get(url, headers: {
      'User-Agent': 'noctorium-installer/1.1',
      if (token.isNotEmpty) 'Authorization': 'Bearer $token',
    });
  }

  /// Streams the APK to the application's own files, reporting progress as it goes.
  ///
  /// Its own folder inside `files`, not the cache: Android may empty the cache at any moment, including
  /// between the download finishing and the installer opening it, and a FileProvider can only share a
  /// path it has been told about.
  Future<File> _download(Asset asset) async {
    final directory = Directory('${(await getApplicationSupportDirectory()).path}/downloads');
    await directory.create(recursive: true);
    final file = File('${directory.path}/${asset.name}');

    final request = http.Request('GET', Uri.parse(asset.url))
      ..headers['User-Agent'] = 'noctorium-installer/1.1';
    final response = await http.Client().send(request);
    if (response.statusCode != 200) {
      throw _Refused('The download was refused (HTTP ${response.statusCode}).');
    }

    final total = response.contentLength ?? asset.size;
    var written = 0;
    final sink = file.openWrite();
    try {
      await for (final chunk in response.stream) {
        sink.add(chunk);
        written += chunk.length;
        if (total > 0 && mounted) {
          setState(() => _progress = written / total);
        }
      }
    } finally {
      await sink.close();
    }
    return file;
  }

  /// Hashed in chunks rather than by reading the whole file into memory: this is a seventeen megabyte
  /// APK on a device that may not have seventeen megabytes to spare.
  Future<String> _sha256(File file) async {
    final digest = await file.openRead().transform(sha256).first;
    return digest.toString();
  }

  /// What a product's button says: what it does, or -- for the product the last attempt was about -- how
  /// that went.
  String _label(Product product) {
    if (product == _product && _stage == Stage.done) return 'Install ${product.title} again';
    if (product == _product && _stage == Stage.failed) return 'Try ${product.title} again';
    return 'Install ${product.title}';
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final quiet = TextStyle(color: scheme.onSurfaceVariant, height: 1.4);
    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.all(28),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text('Noctorium', style: Theme.of(context).textTheme.headlineMedium?.copyWith(fontWeight: FontWeight.bold)),
              const SizedBox(height: 4),
              Text(
                _version == null ? 'Installer' : 'Installer — ${_version!}',
                style: TextStyle(color: scheme.onSurfaceVariant),
              ),
              const SizedBox(height: 28),
              Text(
                _message,
                style: TextStyle(
                  color: _stage == Stage.failed ? scheme.error : scheme.onSurface,
                  height: 1.4,
                ),
              ),
              const SizedBox(height: 20),
              if (_stage == Stage.downloading) LinearProgressIndicator(value: _progress),
              if (_busy && _stage != Stage.downloading) const LinearProgressIndicator(),
              const SizedBox(height: 28),
              FilledButton(
                onPressed: _busy ? null : () => _install(Product.noctorium),
                child: Text(_label(Product.noctorium)),
              ),
              const SizedBox(height: 24),
              // Its own app, beside the player rather than part of it, so its own button -- offered only
              // when the release has it, and saying so when it does not, rather than failing when pressed.
              Text(
                _statsThere
                    ? 'Noctorium Stats: your listening, in figures, from your Noctorium account.'
                    : 'Noctorium Stats is not in ${_latest!.tag} yet.',
                style: quiet,
              ),
              const SizedBox(height: 10),
              FilledButton.tonal(
                onPressed: _busy || !_statsThere ? null : () => _install(Product.stats),
                child: Text(_label(Product.stats)),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A refusal with a reason somebody can act on, as opposed to an exception with a stack trace.
class _Refused implements Exception {
  const _Refused(this.why);
  final String why;
  @override
  String toString() => why;
}
