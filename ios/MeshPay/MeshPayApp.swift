import SwiftUI

@main
struct MeshPayApp: App {
    @State private var model: WalletModel?
    @State private var startupError: String?
    var body: some Scene {
        WindowGroup {
            Group {
                if let model { RootView().environmentObject(model) }
                else if let startupError {
                    ContentUnavailableView("Wallet needs attention", systemImage: "lock.shield", description: Text(startupError))
                } else {
                    ProgressView().task { do { model = try WalletModel() } catch { startupError = error.localizedDescription } }
                }
            }.tint(MeshTheme.green).preferredColorScheme(.light)
        }
    }
}

struct RootView: View {
    @EnvironmentObject var model: WalletModel
    @Environment(\.scenePhase) private var scenePhase
    var body: some View {
        Group {
            if model.saved.onboarded { HomeView() } else { WelcomeView() }
        }
        .alert("A quick heads-up", isPresented: Binding(get: { model.error != nil }, set: { if !$0 { model.error = nil } })) { Button("Got it", role: .cancel) { model.error = nil } } message: { Text(model.error ?? "") }
        .overlay(alignment: .top) {
            if let toast = model.toast {
                HStack(spacing: 9) { Image(systemName: "checkmark.circle.fill"); Text(toast).font(.system(size: 13, weight: .medium)) }
                    .padding(16).foregroundStyle(.white).background(MeshTheme.green, in: Capsule()).padding(.horizontal, 20).padding(.top, 8)
                    .transition(.move(edge: .top).combined(with: .opacity))
                    .task(id: toast) { try? await Task.sleep(for: .seconds(4)); if model.toast == toast { model.toast = nil } }
            }
        }
        .animation(.easeInOut(duration: 0.25), value: model.toast)
        .onChange(of: scenePhase) { _, phase in if phase == .active { model.rebroadcast(); Task { await model.sync() } } }
        .task { await model.sync() }
    }
}

struct WelcomeView: View {
    @EnvironmentObject var model: WalletModel
    @State private var name = ""
    @State private var server = "http://localhost:8787"
    @State private var showServer = false
    @State private var starting = false
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 10) { MeshMark(); Text("mesh").font(.system(size: 32, weight: .semibold, design: .rounded)).tracking(-1) }.padding(.top, 30)
                Spacer(minLength: 45)
                ZStack {
                    ForEach(0..<3) { i in Circle().stroke(MeshTheme.green.opacity(0.07 + Double(i) * 0.025), lineWidth: 1).frame(width: CGFloat(140 + i * 75)) }
                    Circle().fill(MeshTheme.lime).frame(width: 118, height: 118)
                    Image(systemName: "arrow.up.right").font(.system(size: 43, weight: .light)).rotationEffect(.degrees(0))
                    Avatar(name: "S", size: 44).offset(x: 114, y: -55)
                    Avatar(name: "A", size: 38, index: 1).offset(x: -113, y: 45)
                }.frame(maxWidth: .infinity).frame(height: 280)
                Text("Good company.\nMoney, made easy.").font(.system(size: 42, weight: .medium, design: .rounded)).tracking(-1.8).lineSpacing(-1).fixedSize(horizontal: false, vertical: true).padding(.top, 22)
                Text("Pay the people right beside you.\nNo searching. No handles. Just nearby.").font(.system(size: 16)).foregroundStyle(MeshTheme.muted).lineSpacing(5).padding(.top, 16)
                TextField("What should friends call you?", text: $name).textContentType(.givenName).autocorrectionDisabled().padding(19).background(.white, in: RoundedRectangle(cornerRadius: 20)).padding(.top, 30).accessibilityIdentifier("onboarding.name")
                Button { showServer.toggle() } label: { HStack { Text("Development server"); Spacer(); Image(systemName: showServer ? "chevron.up" : "chevron.down") }.font(.system(size: 12)).foregroundStyle(MeshTheme.muted).padding(.vertical, 16) }
                if showServer { TextField("Server address", text: $server).keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled().padding(16).background(.white, in: RoundedRectangle(cornerRadius: 16)) }
                Button { starting = true; Task { await model.finishOnboarding(name: name, server: server); starting = false } } label: { HStack { Text(starting ? "Opening your wallet…" : "Let's get nearby"); Image(systemName: "arrow.right") } }.buttonStyle(PrimaryButton()).disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || starting).opacity(name.isEmpty ? 0.5 : 1).padding(.top, 5)
                Text("SANDBOX · TEST MONEY ONLY").font(.system(size: 10, weight: .medium, design: .monospaced)).tracking(1.5).foregroundStyle(MeshTheme.muted).frame(maxWidth: .infinity).padding(.vertical, 22)
            }.padding(.horizontal, 27)
        }.background(MeshTheme.background).foregroundStyle(MeshTheme.ink)
    }
}
