import SwiftUI

enum MeshTheme {
    static let background = Color(hex: 0xF6F7F2)
    static let ink = Color(hex: 0x18392E)
    static let muted = Color(hex: 0x77877E)
    static let green = Color(hex: 0x245C45)
    static let lime = Color(hex: 0xD4F582)
    static let line = Color(hex: 0xE5E9E0)
    static let soft = Color(hex: 0xEBF0E6)
    static let mono = Font.system(.caption, design: .monospaced)
}
extension Color {
    init(hex: UInt32) { self.init(.sRGB, red: Double((hex >> 16) & 0xff) / 255, green: Double((hex >> 8) & 0xff) / 255, blue: Double(hex & 0xff) / 255, opacity: 1) }
}
struct PrimaryButton: ButtonStyle {
    var light = false
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.font(.system(size: 16, weight: .semibold))
            .padding(.horizontal, 18)
            .frame(maxWidth: .infinity).frame(height: 58)
            .foregroundStyle(light ? MeshTheme.ink : .white)
            .background(light ? MeshTheme.lime : MeshTheme.green, in: RoundedRectangle(cornerRadius: 21))
            .opacity(configuration.isPressed ? 0.75 : 1)
            .scaleEffect(configuration.isPressed ? 0.985 : 1)
    }
}
struct Avatar: View {
    let name: String
    var size: CGFloat = 52
    var index = 0
    private var fill: Color { [Color(hex: 0xE9EACF), Color(hex: 0xE2E7EF), Color(hex: 0xEADFD5), Color(hex: 0xDDE9DA)][abs(index) % 4] }
    var body: some View {
        Text(name.prefix(1).uppercased()).font(.system(size: size * 0.37, weight: .medium, design: .rounded))
            .foregroundStyle(MeshTheme.ink).frame(width: size, height: size)
            .background(fill, in: Circle())
            .accessibilityLabel(name)
    }
}
struct MeshMark: View {
    var size: CGFloat = 32
    var color = MeshTheme.ink
    var body: some View {
        Canvas { context, canvas in
            let points = [CGPoint(x: 0.16, y: 0.3), CGPoint(x: 0.5, y: 0.14), CGPoint(x: 0.84, y: 0.3), CGPoint(x: 0.16, y: 0.7), CGPoint(x: 0.5, y: 0.86), CGPoint(x: 0.84, y: 0.7)].map { CGPoint(x: $0.x * canvas.width, y: $0.y * canvas.height) }
            var path = Path()
            for (a,b) in [(0,1),(1,2),(0,3),(3,4),(4,5),(2,5),(0,4),(1,5),(1,3),(2,4)] { path.move(to: points[a]); path.addLine(to: points[b]) }
            context.stroke(path, with: .color(color.opacity(0.7)), lineWidth: 1.4)
            for p in points { context.fill(Path(ellipseIn: CGRect(x: p.x - 2.5, y: p.y - 2.5, width: 5, height: 5)), with: .color(color)) }
        }.frame(width: size, height: size).accessibilityHidden(true)
    }
}
struct SectionLabel: View {
    let text: String
    var body: some View { Text(text).font(.system(size: 11, weight: .semibold, design: .monospaced)).tracking(1.8).foregroundStyle(MeshTheme.muted) }
}
struct CloseButton: View {
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        Button { dismiss() } label: { Image(systemName: "xmark").font(.system(size: 14, weight: .semibold)).frame(width: 36, height: 36).background(MeshTheme.soft, in: Circle()) }.accessibilityLabel("Close")
    }
}
