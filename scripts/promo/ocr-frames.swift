// 動画フレームの全数 OCR（#470 / #1081 の PII 検査用）。
//
// Vision の VNRecognizeTextRequest で各 PNG のテキストを読み取り、
//   <ファイル名>\t<認識文字列>
// を 1 行ずつ標準出力へ出す。日本語 + 英語を認識対象にし、精度優先（.accurate）で回す。
// 出力を pii-scan.sh へ渡し、メールアドレス・実ユーザー名・ホスト名・トークン等の
// パターンで機械検査する（値そのものは報告へ載せない）。
//
// 使い方:
//   ocr-frames <ディレクトリ or PNG...>
//   ディレクトリを渡すと配下の *.png をファイル名順に処理する
import Foundation
import Vision
import ImageIO

let args = Array(CommandLine.arguments.dropFirst())
guard !args.isEmpty else {
    FileHandle.standardError.write("usage: ocr-frames <dir|png...>\n".data(using: .utf8)!)
    exit(2)
}

var files: [String] = []
let fm = FileManager.default
for a in args {
    var isDir: ObjCBool = false
    if fm.fileExists(atPath: a, isDirectory: &isDir), isDir.boolValue {
        let names = (try? fm.contentsOfDirectory(atPath: a)) ?? []
        for n in names.sorted() where n.lowercased().hasSuffix(".png") {
            files.append((a as NSString).appendingPathComponent(n))
        }
    } else {
        files.append(a)
    }
}

var failures = 0
for path in files {
    guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
        let img = CGImageSourceCreateImageAtIndex(src, 0, nil)
    else {
        FileHandle.standardError.write("ocr-frames: 読めない: \(path)\n".data(using: .utf8)!)
        failures += 1
        continue
    }
    let req = VNRecognizeTextRequest()
    req.recognitionLevel = .accurate
    req.usesLanguageCorrection = false  // 補正で別の語へ化けると検査の意味が薄れる
    req.recognitionLanguages = ["ja-JP", "en-US"]
    let handler = VNImageRequestHandler(cgImage: img, options: [:])
    do {
        try handler.perform([req])
    } catch {
        FileHandle.standardError.write("ocr-frames: 失敗: \(path): \(error)\n".data(using: .utf8)!)
        failures += 1
        continue
    }
    let name = (path as NSString).lastPathComponent
    for obs in req.results ?? [] {
        guard let cand = obs.topCandidates(1).first else { continue }
        let text = cand.string.replacingOccurrences(of: "\t", with: " ")
        print("\(name)\t\(text)")
    }
}
exit(failures == 0 ? 0 : 1)
