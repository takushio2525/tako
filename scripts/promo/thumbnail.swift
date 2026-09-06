// 解説動画 (#1081) の YouTube サムネイル（1280x720）を描く。
// 背景に実収録のフレーム（PNG）を敷き、左側を暗く落として大きな見出しを載せる。
//
// 使い方:
//   thumbnail <出力.png> <背景.png> <見出し 1 行目> [見出し 2 行目] [小ラベル] [--right]
//   --right を付けると背景フレームの見せる側を右寄せにし、文字を左に置く（既定も左置き）
// 絵文字は使わない（ブランド方針）。
import AppKit
import CoreGraphics
import CoreText
import Foundation

var args = Array(CommandLine.arguments.dropFirst())
let flip = args.contains("--right")
args.removeAll { $0 == "--right" }
guard args.count >= 3 else {
    FileHandle.standardError.write(
        "usage: thumbnail <out.png> <bg.png> <line1> [line2] [label] [--right]\n".data(using: .utf8)!)
    exit(2)
}
let outPath = args[0], bgPath = args[1]
let line1 = args[2]
let line2 = args.count >= 4 ? args[3] : ""
let label = args.count >= 5 ? args[4] : ""
let W = 1280, H = 720

guard let bgImg = NSImage(contentsOfFile: bgPath),
      let bgCG = bgImg.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    FileHandle.standardError.write("thumbnail: 背景を読めない\n".data(using: .utf8)!)
    exit(1)
}
let cs = CGColorSpaceCreateDeviceRGB()
guard let ctx = CGContext(data: nil, width: W, height: H, bitsPerComponent: 8, bytesPerRow: 0,
                          space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { exit(1) }

// 背景: フレームを高さいっぱいで置き（16:9 なら幅も一致）、少し右へずらして文字の場所を空ける
let scale = Double(H) / Double(bgCG.height)
let bw = Double(bgCG.width) * scale
let bx = flip ? Double(W) - bw + Double(W) * 0.22 : Double(W) * 0.30
ctx.setFillColor(CGColor(red: 0.051, green: 0.067, blue: 0.090, alpha: 1))
ctx.fill(CGRect(x: 0, y: 0, width: W, height: H))
ctx.draw(bgCG, in: CGRect(x: bx, y: 0, width: bw, height: Double(H)))
// 左 58% を暗くする水平グラデーション（文字の可読性）
let dark = CGColor(red: 0.051, green: 0.067, blue: 0.090, alpha: 0.97)
let clear = CGColor(red: 0.051, green: 0.067, blue: 0.090, alpha: 0.0)
if let g = CGGradient(colorsSpace: cs, colors: [dark, dark, clear] as CFArray, locations: [0, 0.42, 0.72]) {
    ctx.drawLinearGradient(g, start: CGPoint(x: 0, y: 0), end: CGPoint(x: Double(W), y: 0), options: [])
}

func text(_ s: String, size: Double, weight: NSFont.Weight, color: NSColor) -> (CTLine, Double, Double) {
    let font = NSFont.systemFont(ofSize: size, weight: weight)
    let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color]
    let line = CTLineCreateWithAttributedString(NSAttributedString(string: s, attributes: attrs))
    var a: CGFloat = 0, d: CGFloat = 0
    let w = CTLineGetTypographicBounds(line, &a, &d, nil)
    return (line, Double(w), Double(a + d))
}
func drawLine(_ l: CTLine, x: Double, y: Double) {
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -3), blur: 12, color: NSColor(calibratedWhite: 0, alpha: 0.85).cgColor)
    ctx.textPosition = CGPoint(x: x, y: y)
    CTLineDraw(l, ctx)
    ctx.restoreGState()
}

let margin = 64.0
let accent = NSColor(calibratedRed: 0.93, green: 0.35, blue: 0.55, alpha: 1)
// 小ラベル（上）
if !label.isEmpty {
    let (l, w, h) = text(label, size: 30, weight: .semibold, color: .white)
    let pad = 14.0
    let rect = CGRect(x: margin, y: Double(H) - margin - h - pad, width: w + pad * 2, height: h + pad)
    ctx.setFillColor(accent.cgColor)
    ctx.addPath(CGPath(roundedRect: rect, cornerWidth: 8, cornerHeight: 8, transform: nil)); ctx.fillPath()
    drawLine(l, x: margin + pad, y: rect.minY + pad * 0.55)
}
// 見出し 2 行（大きく・太く）。幅に収まるまで縮める
var size1 = 104.0
var t1 = text(line1, size: size1, weight: .heavy, color: .white)
while t1.1 > Double(W) * 0.66 && size1 > 40 { size1 -= 4; t1 = text(line1, size: size1, weight: .heavy, color: .white) }
var size2 = 72.0
var t2 = text(line2, size: size2, weight: .bold, color: NSColor(calibratedRed: 1.0, green: 0.86, blue: 0.40, alpha: 1))
while !line2.isEmpty && t2.1 > Double(W) * 0.66 && size2 > 30 { size2 -= 4; t2 = text(line2, size: size2, weight: .bold, color: NSColor(calibratedRed: 1.0, green: 0.86, blue: 0.40, alpha: 1)) }
let baseline2 = Double(H) * 0.30
let baseline1 = baseline2 + (line2.isEmpty ? 0 : t2.2 * 1.05) + size1 * 0.25
drawLine(t1.0, x: margin, y: baseline1)
if !line2.isEmpty { drawLine(t2.0, x: margin, y: baseline2) }
// 左下: 製品名
let (brand, _, _) = text("tako", size: 44, weight: .bold, color: NSColor(calibratedWhite: 1, alpha: 0.9))
drawLine(brand, x: margin, y: margin * 0.9)

guard let image = ctx.makeImage(),
      let dest = CGImageDestinationCreateWithURL(URL(fileURLWithPath: outPath) as CFURL, "public.png" as CFString, 1, nil) else { exit(1) }
CGImageDestinationAddImage(dest, image, nil)
guard CGImageDestinationFinalize(dest) else { exit(1) }
