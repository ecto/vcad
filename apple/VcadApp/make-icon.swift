#!/usr/bin/env swift
// Draws the app and document icons: a rounded squircle with a soft gradient
// and an isometric cube in the brand pink, as .icns. Run by bundle.sh; usage:
//   swift make-icon.swift <output-dir>
import AppKit

let outDir = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "dist"
try? FileManager.default.createDirectory(atPath: outDir, withIntermediateDirectories: true)

func draw(size: CGFloat, document: Bool) -> NSImage {
    let img = NSImage(size: NSSize(width: size, height: size))
    img.lockFocus()
    let s = size
    if document {
        // A page with a folded corner, the standard document silhouette.
        let page = NSBezierPath()
        let inset = s * 0.12, fold = s * 0.22
        page.move(to: NSPoint(x: inset, y: inset))
        page.line(to: NSPoint(x: s - inset - fold, y: inset))
        page.line(to: NSPoint(x: s - inset, y: inset + fold))
        page.line(to: NSPoint(x: s - inset, y: s - inset))
        page.line(to: NSPoint(x: inset, y: s - inset))
        page.close()
        NSColor.white.setFill(); page.fill()
        NSColor(white: 0.82, alpha: 1).setStroke(); page.lineWidth = s * 0.012; page.stroke()
    } else {
        let rect = NSRect(x: s * 0.06, y: s * 0.06, width: s * 0.88, height: s * 0.88)
        let squircle = NSBezierPath(roundedRect: rect, xRadius: s * 0.2, yRadius: s * 0.2)
        NSGradient(colors: [NSColor(srgbRed: 0.16, green: 0.17, blue: 0.22, alpha: 1),
                            NSColor(srgbRed: 0.07, green: 0.075, blue: 0.10, alpha: 1)])!
            .draw(in: squircle, angle: -90)
    }
    // Isometric cube: three faces in brand pink, shaded.
    let c = NSPoint(x: s * 0.5, y: s * (document ? 0.47 : 0.5))
    let r = s * (document ? 0.22 : 0.28)
    let top = NSColor(srgbRed: 0.99, green: 0.35, blue: 0.58, alpha: 1)
    let left = NSColor(srgbRed: 0.85, green: 0.12, blue: 0.40, alpha: 1)
    let right = NSColor(srgbRed: 0.68, green: 0.08, blue: 0.32, alpha: 1)
    func pt(_ a: CGFloat, _ b: CGFloat) -> NSPoint { NSPoint(x: c.x + a * r, y: c.y + b * r) }
    let h: CGFloat = 0.5, w: CGFloat = 0.866
    let faces: [(NSColor, [NSPoint])] = [
        (top, [pt(0, 1), pt(w, h), pt(0, 0), pt(-w, h)]),
        (left, [pt(-w, h), pt(0, 0), pt(0, -1), pt(-w, -h)]),
        (right, [pt(0, 0), pt(w, h), pt(w, -h), pt(0, -1)]),
    ]
    for (color, pts) in faces {
        let p = NSBezierPath(); p.move(to: pts[0]); pts.dropFirst().forEach { p.line(to: $0) }; p.close()
        color.setFill(); p.fill()
    }
    img.unlockFocus()
    return img
}

func writeICNS(_ name: String, document: Bool) {
    let iconset = "\(outDir)/\(name).iconset"
    try? FileManager.default.removeItem(atPath: iconset)
    try! FileManager.default.createDirectory(atPath: iconset, withIntermediateDirectories: true)
    for (px, tag) in [(16, "16x16"), (32, "16x16@2x"), (32, "32x32"), (64, "32x32@2x"),
                      (128, "128x128"), (256, "128x128@2x"), (256, "256x256"), (512, "256x256@2x"),
                      (512, "512x512"), (1024, "512x512@2x")] {
        let img = draw(size: CGFloat(px), document: document)
        guard let tiff = img.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let png = rep.representation(using: .png, properties: [:]) else { continue }
        try! png.write(to: URL(fileURLWithPath: "\(iconset)/icon_\(tag).png"))
    }
    let task = Process()
    task.launchPath = "/usr/bin/iconutil"
    task.arguments = ["-c", "icns", iconset, "-o", "\(outDir)/\(name).icns"]
    task.launch(); task.waitUntilExit()
    try? FileManager.default.removeItem(atPath: iconset)
}

writeICNS("AppIcon", document: false)
writeICNS("DocumentIcon", document: true)
print("icons written to \(outDir)")
