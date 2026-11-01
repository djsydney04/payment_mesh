import SwiftUI

struct SettingsView: View {
    @EnvironmentObject var model: WalletModel
    @State private var server = ""
    @State private var busy = false
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 23) {
                HStack { Text("Your little corner").font(.system(size: 28, weight: .medium, design: .rounded)).tracking(-0.8); Spacer(); CloseButton() }.padding(.top, 20)
                HStack(spacing: 16) { Avatar(name: model.saved.name, size: 68, index: 3); VStack(alignment: .leading, spacing: 5) { Text(model.saved.name).font(.system(size: 22, weight: .medium, design: .rounded)); Text("Your nearby identity").font(.system(size: 12)).foregroundStyle(MeshTheme.muted) } }.padding(.vertical, 14)
                VStack(alignment: .leading, spacing: 13) {
                    SectionLabel(text: "CHECK IT WITH A FRIEND")
                    Text(model.profile.fingerprint).font(.system(size: 22, weight: .medium, design: .monospaced)).textSelection(.enabled)
                    Text("Ask a friend to compare this code before you pay. It identifies your wallet, even if someone uses the same name.").font(.system(size: 12)).foregroundStyle(MeshTheme.muted).lineSpacing(4)
                }.padding(22).background(.white, in: RoundedRectangle(cornerRadius: 23))
                VStack(alignment: .leading, spacing: 13) {
                    SectionLabel(text: "CONNECTION")
                    Label(model.bluetoothStatus, systemImage: "dot.radiowaves.left.and.right").font(.system(size: 13))
                    Label(model.online ? "Connected to your ledger" : "Offline · payments will wait to sync", systemImage: model.online ? "checkmark.icloud" : "icloud.slash").font(.system(size: 13))
                    Button { Task { await model.sync() } } label: { Text(model.syncing ? "Syncing…" : "Sync now").font(.system(size: 13, weight: .medium)) }.disabled(model.syncing)
                }.padding(22).background(.white, in: RoundedRectangle(cornerRadius: 23))
                DisclosureGroup("Development settings") {
                    VStack(alignment: .leading, spacing: 15) {
                        TextField("Server address", text: $server).keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled().font(.system(size: 13, design: .monospaced)).padding(14).background(.white, in: RoundedRectangle(cornerRadius: 12))
                        Button(busy ? "Connecting…" : "Save server") { busy = true; Task { await model.updateServer(server); busy = false } }.disabled(busy || model.isPreview)
                        Button("Add test funds once") { Task { await model.fundDemo() } }.disabled(!model.online || model.isPreview)
                        Text("On a real iPhone, use your Mac's local network address instead of localhost. The server must be started with MESH_BIND=0.0.0.0:8787.").font(.system(size: 11)).foregroundStyle(MeshTheme.muted).lineSpacing(3)
                    }.padding(.top, 15)
                }.font(.system(size: 13)).padding(.horizontal, 5)
                Text("Keys stay in this iPhone's Keychain. There is no wallet recovery yet.\n\nMesh v0.1 · USD sandbox\nOffline payments are instructions, and may fail when synced. \(model.isBankBacked ? "Bank transfers use Dwolla Sandbox." : "Bank payouts are simulated locally.")")
                    .font(.system(size: 11)).foregroundStyle(MeshTheme.muted).lineSpacing(4).padding(.horizontal, 5).padding(.bottom, 30)
            }.padding(.horizontal, 26)
        }.background(MeshTheme.background).foregroundStyle(MeshTheme.ink).onAppear { server = model.saved.serverUrl }.presentationDragIndicator(.visible)
    }
}
