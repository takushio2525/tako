// 紹介動画 (#470) のテロップ画像を作る。
// ffmpeg の drawtext は本環境の ffmpeg に含まれていない（libfreetype 無しビルド）ため、
// CoreText で透過 PNG を描いて overlay フィルタで合成する。
//
// 使い方:
//   caption <出力.png> <幅> <フォントpx> <本文> [副題]
// 本文は太字・副題は細字で 1 段下に描く。背景は透過、文字には薄い影を付ける。
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
let padding = size * 0.9
let lineGap = subtitle.isEmpty ? 0 : subSize * 1.5
let height = Int(size * 1.5 + lineGap + padding)

let cs = CGColorSpaceCreateDeviceRGB()
guard
    let ctx = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
else { exit(1) }
ctx.clear(CGRect(x: 0, y: 0, width: width, height: height))

/// 中央揃えで 1 行描く。影を先に落としてから本体を描く
func draw(_ s: String, fontSize: Double, weight: NSFont.Weight, y: Double, alpha: Double) {
    guard !s.isEmpty else { return }
    let font = NSFont.systemFont(ofSize: fontSize, weight: weight)
    let attrs: [NSAttributedString.Key: Any] = [
        .font: font,
        .foregroundColor: NSColor(calibratedWhite: 1.0, alpha: alpha),
    ]
    let attr = NSAttributedString(string: s, attributes: attrs)
    let line = CTLineCreateWithAttributedString(attr)
    let bounds = CTLineGetBoundsWithOptions(line, .useOpticalBounds)
    let x = (Double(width) - Double(bounds.width)) / 2.0

    // 影（下方向に少しずらした半透明の黒）
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -2), blur: 8,
                  color: NSColor(calibratedWhite: 0, alpha: 0.85).cgColor)
    ctx.textPosition = CGPoint(x: x, y: y)
    CTLineDraw(line, ctx)
    ctx.restoreGState()
}

draw(text, fontSize: size, weight: .semibold,
     y: Double(height) - size * 1.15, alpha: 1.0)
if !subtitle.isEmpty {
    draw(subtitle, fontSize: subSize, weight: .regular,
         y: Double(height) - size * 1.15 - lineGap, alpha: 0.82)
}

guard let image = ctx.makeImage() else { exit(1) }
let url = URL(fileURLWithPath: outPath)
guard
    let dest = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil)
else { exit(1) }
CGImageDestinationAddImage(dest, image, nil)
guard CGImageDestinationFinalize(dest) else { exit(1) }
