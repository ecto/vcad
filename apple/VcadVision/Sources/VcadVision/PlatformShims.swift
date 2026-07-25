import SwiftUI
import UIKit

// The shared sources (symlinked from ../VcadApp) speak AppKit's dialect.
// visionOS is UIKit-flavored; these shims keep the shared code word-for-word.

typealias NSColor = UIColor

/// Stand-in for NSColorSpace at the one shared call site
/// (`part.color.usingColorSpace(.sRGB)`): UIColor components are already fine.
struct PortedColorSpace {
    static let sRGB = PortedColorSpace()
}

extension UIColor {
    /// AppKit spelling; UIKit's device-RGB init is close enough on visionOS.
    convenience init(srgbRed r: CGFloat, green g: CGFloat, blue b: CGFloat, alpha a: CGFloat) {
        self.init(red: r, green: g, blue: b, alpha: a)
    }

    /// AppKit color-space accessors the shared code touches. UIColor's
    /// getRed(_:green:blue:alpha:) already handles conversion.
    var redComponent: CGFloat { rgba.0 }
    var greenComponent: CGFloat { rgba.1 }
    var blueComponent: CGFloat { rgba.2 }
    var alphaComponent: CGFloat { rgba.3 }
    func usingColorSpace(_ space: PortedColorSpace) -> UIColor? { self }

    private var rgba: (CGFloat, CGFloat, CGFloat, CGFloat) {
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        getRed(&r, green: &g, blue: &b, alpha: &a)
        return (r, g, b, a)
    }
}
