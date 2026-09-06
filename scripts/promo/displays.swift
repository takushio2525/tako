// 接続中のディスプレイを 1 行ずつ「displayID x y w h pxW pxH main|secondary」で出す。
// x y w h はポイント（CoreGraphics のグローバル座標 = メインディスプレイの左上が原点・y は下向き）、
// pxW pxH は実ピクセル（HiDPI なら w h の 2 倍）。
//
// 仮想ディスプレイ収録（#1081）で、隔離 tako の窓を置く先（仮想ディスプレイの矩形）を決め、
// 収録前後に「窓がその矩形の中にある」「2x で撮れる」ことを機械確認するのに使う。
// tako-app の layout.json の `window` フレームも同じ座標系（GPUI がメインスクリーンの高さで
// y を反転して Cocoa 座標へ直す）なので、ここで得た x/y をそのまま seed に使える。
import CoreGraphics
import Foundation

var count: UInt32 = 0
var ids = [CGDirectDisplayID](repeating: 0, count: 16)
guard CGGetActiveDisplayList(16, &ids, &count) == .success else { exit(1) }
for i in 0..<Int(count) {
    let d = ids[i]
    let b = CGDisplayBounds(d)
    var pxW = Int(b.width), pxH = Int(b.height)
    if let mode = CGDisplayCopyDisplayMode(d) {
        pxW = mode.pixelWidth
        pxH = mode.pixelHeight
    }
    let kind = CGDisplayIsMain(d) == 1 ? "main" : "secondary"
    print("\(d) \(Int(b.origin.x)) \(Int(b.origin.y)) \(Int(b.width)) \(Int(b.height)) \(pxW) \(pxH) \(kind)")
}
