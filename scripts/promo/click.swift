// 収録中の GUI 操作を「実 OS マウスと同じイベント」で行う（Issue #1081 の GUI モード章）。
//
// なぜ必要か: かんたん表示のボタンは押した瞬間に `begin_pane_settle`（#720）を張るので、
// 「準備中… / AI を起動しています」の画面はボタン押下でしか出ない。CLI で同じコマンドを
// シェルへ送ると起動ログが素通しで見えてしまい、**ユーザーが実際に見る絵と違うものを撮る**
// ことになる。System Events の合成クリックは GPUI に届かない（#1081 の実測）ため、
// CGEvent を HID タップへ流す。
//
// 使い方:
//   click <x> <y> [--hover-ms N] [--no-restore] [--move-only]
//     x / y      … CG のグローバル座標（左上原点・y 下向き。仮想ディスプレイ上でよい）
//     --hover-ms … 押す前にその場へ留まる時間（ホバー状態を作る。既定 250）
//     --move-only … 動かすだけで押さない（カーソルを元の画面へ戻すときに使う）
//     --no-restore … 押した位置にカーソルを置いたままにする（既定は元の位置へ戻す）
//
// **既定で元の位置へ戻す**のは、収録がユーザーのポインタを奪わないようにするため
// （仮想ディスプレイ側に置き去りにすると、ユーザーは見えない画面にカーソルを失う）。
import Foundation
import CoreGraphics

let args = Array(CommandLine.arguments.dropFirst())
guard args.count >= 2, let tx = Double(args[0]), let ty = Double(args[1]) else {
    FileHandle.standardError.write(
        "usage: click <x> <y> [--hover-ms N] [--no-restore] [--move-only]\n".data(using: .utf8)!)
    exit(2)
}
var hoverMs = 250
var restore = true
var moveOnly = false
var i = 2
while i < args.count {
    switch args[i] {
    case "--hover-ms": if i + 1 < args.count { hoverMs = Int(args[i + 1]) ?? hoverMs; i += 1 }
    case "--no-restore": restore = false
    case "--move-only": moveOnly = true; restore = false
    default: break
    }
    i += 1
}

let target = CGPoint(x: tx, y: ty)
let saved = CGEvent(source: nil)?.location ?? CGPoint(x: 0, y: 0)
let src = CGEventSource(stateID: .hidSystemState)

// GPUI の hover は MouseMove で立つので、数歩に分けて動かす（1 回のワープだと
// 移動イベントが 1 つしか流れず、ホバー状態が付かないことがある）
let steps = 6
for s in 1...steps {
    let f = Double(s) / Double(steps)
    let p = CGPoint(x: saved.x + (target.x - saved.x) * f, y: saved.y + (target.y - saved.y) * f)
    CGWarpMouseCursorPosition(p)
    CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: p, mouseButton: .left)?
        .post(tap: .cghidEventTap)
    usleep(20_000)
}
usleep(UInt32(hoverMs) * 1000)

if moveOnly {
    print("moved \(Int(tx)),\(Int(ty)) from=\(Int(saved.x)),\(Int(saved.y))")
    exit(0)
}

CGEvent(mouseEventSource: src, mouseType: .leftMouseDown, mouseCursorPosition: target, mouseButton: .left)?
    .post(tap: .cghidEventTap)
usleep(90_000)
CGEvent(mouseEventSource: src, mouseType: .leftMouseUp, mouseCursorPosition: target, mouseButton: .left)?
    .post(tap: .cghidEventTap)
usleep(120_000)

if restore {
    CGWarpMouseCursorPosition(saved)
    CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: saved, mouseButton: .left)?
        .post(tap: .cghidEventTap)
}
print("clicked \(Int(tx)),\(Int(ty)) restored=\(restore) from=\(Int(saved.x)),\(Int(saved.y))")
