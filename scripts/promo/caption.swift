// 紹介動画 (#470) のテロップ画像を作る。
// ffmpeg の drawtext は本環境の ffmpeg に含まれていない（libfreetype 無しビルド）ため、
// CoreText で透過 PNG を描いて overlay フィルタで合成する。
//
// 使い方:
//   caption <出力.png> <幅> <フォントpx> <本文> [副題]
// 本文は太字・副題は細字で 1 段下に描く。
//
// v2（2026-07-24）: 文字だけを載せると背景の UI（ログ・コード・プレビュー）と重なって
// 読めなかったため、文字の外接矩形に合わせた**半透明の暗色パネル**を敷く。
// パネル幅はテキスト幅 + 左右パディングで、キャンバス中央に置く。
// キャンバス自体は動画幅いっぱい（overlay の x=0 前提）で透過のまま。
import AppKit
import CoreGraphics
import CoreText
import Foundation

let args = CommandLine.arguments
guard args.count >= 5, let width = Int(args[2]), let size = Double(args[3]) else {
    FileHandle.standardError.write(
        "usage: caption <out.png> <width> <fontpx> <text> [subtitle]\n".data(using: .utf8)!)
    exit(2)
}
let outPath = args[1]
let text = args[4]
let subtitle = args.count >= 6 ? args[5] : ""

let subSize = size * 0.52

// パネルの見た目（背景映像とのコントラストを稼ぐための値。実フレームで調整済み）
let panelAlpha = 0.84  // 暗色パネルの不透明度（背景のログ・コードを透かさない濃さ）
let cornerRadius = size * 0.28
let padH = size * 0.62  // 左右パディング
let padV = size * 0.34  // 上下パディング
let gap = subtitle.isEmpty ? 0.0 : size * 0.26  // 本文と副題の間隔
let margin = size * 0.30  // パネル外の余白（影のはみ出し分）

/// 1 行ぶんの CTLine と寸法
struct Line {
    let line: CTLine
    let width: Double
    let ascent: Double
    let descent: Double
}

func makeLine(_ s: String, fontSize: Double, weight: NSFont.Weight, alpha: Double) -> Line? {
    guard !s.isEmpty else { return nil }
    let font = NSFont.systemFont(ofSize: fontSize, weight: weight)
    let attrs: [NSAttributedString.Key: Any] = [
        .font: font,
        .foregroundColor: NSColor(calibratedWhite: 1.0, alpha: alpha),
    ]
    let ct = CTLineCreateWithAttributedString(NSAttributedString(string: s, attributes: attrs))
    var ascent: CGFloat = 0
    var descent: CGFloat = 0
    var leading: CGFloat = 0
    let w = CTLineGetTypographicBounds(ct, &ascent, &descent, &leading)
    return Line(line: ct, width: Double(w), ascent: Double(ascent), descent: Double(descent))
}

guard let main = makeLine(text, fontSize: size, weight: .semibold, alpha: 1.0) else {
    FileHandle.standardError.write("caption: 本文が空\n".data(using: .utf8)!)
    exit(2)
}
let sub = makeLine(subtitle, fontSize: subSize, weight: .regular, alpha: 0.92)

let contentW = max(main.width, sub?.width ?? 0)
let contentH =
    main.ascent + main.descent + (sub.map { gap + $0.ascent + $0.descent } ?? 0)
let panelW = min(Double(width) - margin * 2, contentW + padH * 2)
let panelH = contentH + padV * 2
let height = Int((panelH + margin * 2).rounded(.up))

let cs = CGColorSpaceCreateDeviceRGB()
guard
    let ctx = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
else { exit(1) }
ctx.clear(CGRect(x: 0, y: 0, width: width, height: height))

// ── 背景パネル ────────────────────────────────────────────────────
let panelRect = CGRect(
    x: (Double(width) - panelW) / 2.0, y: margin, width: panelW, height: panelH)
let panelPath = CGPath(
    roundedRect: panelRect, cornerWidth: cornerRadius, cornerHeight: cornerRadius,
    transform: nil)

ctx.saveGState()
// パネル自体にも影を落として、明るい背景の上でも縁が溶けないようにする
ctx.setShadow(
    offset: CGSize(width: 0, height: -3), blur: 18,
    color: NSColor(calibratedWhite: 0, alpha: 0.55).cgColor)
ctx.addPath(panelPath)
// tako のダーク背景（#0d1117 相当）に寄せた暗色。純黒より画面に馴染む
ctx.setFillColor(
    NSColor(calibratedRed: 0.04, green: 0.05, blue: 0.07, alpha: panelAlpha).cgColor)
ctx.fillPath()
ctx.restoreGState()

// 縁の 1px ハイライト（暗い背景に載ったときの輪郭）
ctx.saveGState()
ctx.addPath(panelPath)
ctx.setStrokeColor(NSColor(calibratedWhite: 1.0, alpha: 0.14).cgColor)
ctx.setLineWidth(1.5)
ctx.strokePath()
ctx.restoreGState()

// ── 文字 ──────────────────────────────────────────────────────────
func draw(_ l: Line, baseline: Double) {
    ctx.saveGState()
    ctx.setShadow(
        offset: CGSize(width: 0, height: -1), blur: 3,
        color: NSColor(calibratedWhite: 0, alpha: 0.8).cgColor)
    ctx.textPosition = CGPoint(x: (Double(width) - l.width) / 2.0, y: baseline)
    CTLineDraw(l.line, ctx)
    ctx.restoreGState()
}

if let sub {
    draw(sub, baseline: margin + padV + sub.descent)
    draw(main, baseline: margin + padV + sub.descent + sub.ascent + gap + main.descent)
} else {
    draw(main, baseline: margin + padV + main.descent)
}

guard let image = ctx.makeImage() else { exit(1) }
let url = URL(fileURLWithPath: outPath)
guard
    let dest = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil)
else { exit(1) }
CGImageDestinationAddImage(dest, image, nil)
guard CGImageDestinationFinalize(dest) else { exit(1) }
