// 収録中に「かんたん表示のチャット入力欄へ人が打つ」絵を作る（Issue #1081 の GUI モード章）。
//
// `CGEventPostToPid` は**対象プロセスへ直接**イベントを渡すので、隔離 tako を前面化
// しなくても入力が届く（= ユーザーのキーフォーカスを奪わない。#1141 の方針）。
// `tako send` はペインのシェル（claude の TUI）へ送る経路なので、チャット入力欄に
// 文字が入っていく絵にはならない。
//
// 使い方:
//   keytype <pid> <text> [--per-char-ms N] [--return] [--backspaces N]
//     --per-char-ms … 1 文字あたりの間隔（既定 55ms。人が打つ速さに見せる）
//     --return      … 打ち終わりに Enter（送信）
//     --backspaces  … 打つ前に BS を N 回（前の残りを消す。空欄なら無害）
//
// 実測（2026-09-09）: `keyboardSetUnicodeString` で日本語がそのまま入る（IME を経由しない）。
// クリック直後の 1 打目が化けることがあったので、呼び出し側は打ったあとに OCR で
// 入力欄の内容を確かめてから Enter を送る形にしてある（record-explainer.sh の promo_type_verified）。
import Foundation
import CoreGraphics

let a = Array(CommandLine.arguments.dropFirst())
guard a.count >= 2, let pid = pid_t(a[0]) else {
    FileHandle.standardError.write(
        "usage: keytype <pid> <text> [--per-char-ms N] [--return] [--backspaces N]\n".data(using: .utf8)!)
    exit(2)
}
let text = a[1]
var perCharMs = 55
var sendReturn = false
var backspaces = 0
var i = 2
while i < a.count {
    switch a[i] {
    case "--per-char-ms": if i + 1 < a.count { perCharMs = Int(a[i + 1]) ?? perCharMs; i += 1 }
    case "--return": sendReturn = true
    case "--backspaces": if i + 1 < a.count { backspaces = Int(a[i + 1]) ?? 0; i += 1 }
    default: break
    }
    i += 1
}

let src = CGEventSource(stateID: .hidSystemState)

// 51 = Delete（BS）
for _ in 0..<backspaces {
    CGEvent(keyboardEventSource: src, virtualKey: 51, keyDown: true)?.postToPid(pid)
    usleep(8_000)
    CGEvent(keyboardEventSource: src, virtualKey: 51, keyDown: false)?.postToPid(pid)
    usleep(35_000)
}

for ch in text.unicodeScalars.map({ String($0) }) {
    var utf16 = Array(ch.utf16)
    if let down = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: true) {
        down.keyboardSetUnicodeString(stringLength: utf16.count, unicodeString: &utf16)
        down.postToPid(pid)
    }
    usleep(8_000)
    if let up = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: false) {
        up.keyboardSetUnicodeString(stringLength: utf16.count, unicodeString: &utf16)
        up.postToPid(pid)
    }
    usleep(UInt32(perCharMs) * 1000)
}

if sendReturn {
    usleep(200_000)
    // 36 = Return
    CGEvent(keyboardEventSource: src, virtualKey: 36, keyDown: true)?.postToPid(pid)
    usleep(30_000)
    CGEvent(keyboardEventSource: src, virtualKey: 36, keyDown: false)?.postToPid(pid)
}
print("typed \(text.count) chars to pid \(pid) return=\(sendReturn) backspaces=\(backspaces)")
