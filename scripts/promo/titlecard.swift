// 解説動画 (#1081) の章タイトルカード / クロージングカードを描く。
// caption.swift（下段テロップ）と同じ CoreText 描画で、こちらは**全面**の不透明カード。
//
// 使い方:
//   titlecard <出力.png> <幅> <高さ> <見出し> [副題] [右上の小ラベル] [脚注行]
// 例:
//   titlecard card.png 1920 1080 "tako とは何か" "AI エージェント時代の GUI ターミナル" "1 / 8" "github.com/takushio2525/tako"
//
// 配色は tako のダークテーマ（#0d1117 系）に寄せる。絵文字は使わない（ブランド方針）。
import AppKit
import CoreGraphics
import CoreText
import Foundation

let args = CommandLine.arguments
guard args.count >= 5, let width = Int(args[2]), let height = Int(args[3]) else {
    FileHandle.standardError.write(
        "usage: titlecard <out.png> <w> <h> <title> [subtitle] [label] [footer]\n".data(using: .utf8)!)
    exit(2)
}
let outPath = args[1]
let title = args[4]
let subtitle = args.count >= 6 ? args[5] : ""
let label = args.count >= 7 ? args[6] : ""
let footer = args.count >= 8 ? args[7] : ""

let cs = CGColorSpaceCreateDeviceRGB()
guard
    let ctx = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
else { exit(1) }

// ── 背景: 縦グラデーション（上 #0d1117 → 下 #161b22）+ 左端のアクセント帯 ──
let top = CGColor(red: 0.051, green: 0.067, blue: 0.090, alpha: 1)
let bottom = CGColor(red: 0.086, green: 0.106, blue: 0.133, alpha: 1)
if let grad = CGGradient(colorsSpace: cs, colors: [top, bottom] as CFArray, locations: [0, 1]) {
    ctx.drawLinearGradient(
        grad, start: CGPoint(x: 0, y: CGFloat(height)), end: CGPoint(x: 0, y: 0), options: [])
}
// 薄いグリッド（背景が真っ黒に見えないための質感）
ctx.setStrokeColor(CGColor(red: 1, green: 1, blue: 1, alpha: 0.035))
ctx.setLineWidth(1)
let grid = CGFloat(width) / 24
var gx: CGFloat = 0
while gx <= CGFloat(width) {
    ctx.move(to: CGPoint(x: gx, y: 0)); ctx.addLine(to: CGPoint(x: gx, y: CGFloat(height)))
    gx += grid
}
var gy: CGFloat = 0
while gy <= CGFloat(height) {
    ctx.move(to: CGPoint(x: 0, y: gy)); ctx.addLine(to: CGPoint(x: CGFloat(width), y: gy))
    gy += grid
}
ctx.strokePath()

// アクセント（tako のピンク系）。見出しの左に短い縦バー
let accent = CGColor(red: 0.93, green: 0.35, blue: 0.55, alpha: 1)

func draw(_ s: String, size: Double, weight: NSFont.Weight, color: NSColor, x: Double, baseline: Double,
          maxWidth: Double) -> Double {
    guard !s.isEmpty else { return 0 }
    var fs = size
    var line: CTLine
    var w: Double
    var ascent: CGFloat = 0, descent: CGFloat = 0, leading: CGFloat = 0
    repeat {
        let font = NSFont.systemFont(ofSize: fs, weight: weight)
        let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color]
        line = CTLineCreateWithAttributedString(NSAttributedString(string: s, attributes: attrs))
        w = Double(CTLineGetTypographicBounds(line, &ascent, &descent, &leading))
        if w <= maxWidth || fs < 20 { break }
        fs -= 2
    } while true
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -2), blur: 6,
                  color: NSColor(calibratedWhite: 0, alpha: 0.6).cgColor)
    ctx.textPosition = CGPoint(x: x, y: baseline)
    CTLineDraw(line, ctx)
    ctx.restoreGState()
    return Double(ascent + descent)
}

let W = Double(width), H = Double(height)
let margin = W * 0.09
let titleSize = H * 0.085
let subSize = H * 0.040
let labelSize = H * 0.028
let footSize = H * 0.026

// 見出し（中央やや上）
let titleBase = H * 0.50
let barH = titleSize * 1.05
ctx.setFillColor(accent)
ctx.fill(CGRect(x: margin - titleSize * 0.55, y: titleBase - titleSize * 0.18, width: titleSize * 0.12, height: barH))
_ = draw(title, size: titleSize, weight: .bold, color: .white, x: margin, baseline: titleBase,
         maxWidth: W - margin * 2)
// 副題
_ = draw(subtitle, size: subSize, weight: .regular,
         color: NSColor(calibratedWhite: 1, alpha: 0.82), x: margin,
         baseline: titleBase - subSize * 1.9, maxWidth: W - margin * 2)
// 右上ラベル（章番号など）
if !label.isEmpty {
    let font = NSFont.systemFont(ofSize: labelSize, weight: .medium)
    let attrs: [NSAttributedString.Key: Any] = [
        .font: font, .foregroundColor: NSColor(calibratedWhite: 1, alpha: 0.7)]
    let l = CTLineCreateWithAttributedString(NSAttributedString(string: label, attributes: attrs))
    let lw = CTLineGetTypographicBounds(l, nil, nil, nil)
    ctx.textPosition = CGPoint(x: W - margin - lw, y: H - margin * 0.9)
    CTLineDraw(l, ctx)
}
// 脚注（左下）
_ = draw(footer, size: footSize, weight: .regular,
         color: NSColor(calibratedWhite: 1, alpha: 0.6), x: margin, baseline: margin * 0.8,
         maxWidth: W - margin * 2)

guard let image = ctx.makeImage() else { exit(1) }
guard let dest = CGImageDestinationCreateWithURL(
    URL(fileURLWithPath: outPath) as CFURL, "public.png" as CFString, 1, nil) else { exit(1) }
CGImageDestinationAddImage(dest, image, nil)
guard CGImageDestinationFinalize(dest) else { exit(1) }
