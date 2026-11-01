import SwiftUI

struct HomeView: View {
    @EnvironmentObject var model: WalletModel
    @State private var selectedPerson: NearbyPerson?
    @State private var showBank = false
    @State private var showSettings = false
    @State private var showNearby = false
    @State private var tab = 0
    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 9) {
                MeshMark(size: 29)
                Text("mesh").font(.system(size: 29, weight: .semibold, design: .rounded)).tracking(-1)
                Spacer()
                HStack(spacing: 5) { Circle().fill(MeshTheme.green).frame(width: 5, height: 5); Text(model.isPreview ? "PREVIEW" : "SANDBOX").font(.system(size: 9, weight: .medium, design: .monospaced)).tracking(1) }.padding(.horizontal, 11).padding(.vertical, 8).background(MeshTheme.soft, in: Capsule())
                Button { showSettings = true } label: { Avatar(name: model.saved.name, size: 35, index: 3) }.accessibilityLabel("Wallet settings")
            }.padding(.horizontal, 25).padding(.top, 13).padding(.bottom, 22)
            ScrollView(showsIndicators: false) {
                VStack(alignment: .leading, spacing: 26) {
                    if tab == 0 { balanceCard; nearbySection; activitySection; privacyNote }
                    else { activitySection; privacyNote }
                }.padding(.horizontal, 25).padding(.bottom, 25)
            }.refreshable { await model.sync() }
            HStack {
                navButton("Nearby", icon: "dot.radiowaves.left.and.right", index: 0)
                Spacer()
                HStack(spacing: 5) { Image(systemName: model.online ? "checkmark.icloud" : "icloud.slash"); Text(model.syncing ? "Syncing" : (model.online ? "Up to date" : "Offline")) }.font(.system(size: 10)).foregroundStyle(MeshTheme.muted)
                Spacer()
                navButton("Activity", icon: "clock.arrow.circlepath", index: 1)
            }.padding(.horizontal, 39).padding(.top, 16).padding(.bottom, 7).background(MeshTheme.background.shadow(color: .black.opacity(0.025), radius: 10, y: -5))
        }
        .background(MeshTheme.background).foregroundStyle(MeshTheme.ink)
        .task { if model.bankLinkInterrupted { showBank = true } }
        .sheet(item: $selectedPerson) { SendView(person: $0).environmentObject(model) }
        .sheet(isPresented: $showBank, onDismiss: { model.cancelBankSignIn() }) { BankView().environmentObject(model) }
        .sheet(isPresented: $showSettings) { SettingsView().environmentObject(model) }
        .sheet(isPresented: $showNearby) { nearbySheet }
    }
    private var balanceCard: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("YOUR BALANCE").font(.system(size: 10, weight: .medium, design: .monospaced)).tracking(1.9).foregroundStyle(.white.opacity(0.66))
                Spacer()
                Image(systemName: "wave.3.right").font(.system(size: 16, weight: .light)).foregroundStyle(MeshTheme.lime)
            }
            Text(money(model.availableCents)).font(.system(size: 56, weight: .regular, design: .rounded)).tracking(-2.6).contentTransition(.numericText()).minimumScaleFactor(0.65).lineLimit(1).padding(.top, 18)
            HStack(spacing: 5) { Circle().fill(MeshTheme.lime).frame(width: 4, height: 4); Text(model.pending.isEmpty ? "Ready for a little everyday magic" : "\(model.pending.count) payment\(model.pending.count == 1 ? "" : "s") waiting to sync").font(.system(size: 11)).foregroundStyle(.white.opacity(0.7)) }.padding(.top, 4)
            HStack(spacing: 10) {
                Button { showNearby = true } label: { Label("Pay nearby", systemImage: "arrow.up.right").font(.system(size: 13, weight: .semibold)).frame(maxWidth: .infinity).frame(height: 44).foregroundStyle(MeshTheme.ink).background(MeshTheme.lime, in: RoundedRectangle(cornerRadius: 15)) }.accessibilityIdentifier("home.pay")
                Button { showBank = true } label: { Label("Cash out", systemImage: "building.columns").font(.system(size: 13, weight: .medium)).frame(maxWidth: .infinity).frame(height: 44).foregroundStyle(.white).background(.white.opacity(0.11), in: RoundedRectangle(cornerRadius: 15)) }.accessibilityIdentifier("home.cashout")
            }.padding(.top, 27)
        }.padding(24).foregroundStyle(.white)
            .background {
                ZStack(alignment: .topTrailing) {
                    MeshTheme.ink
                    Circle().stroke(.white.opacity(0.045), lineWidth: 1).frame(width: 220).offset(x: 93, y: -80)
                    Circle().stroke(.white.opacity(0.045), lineWidth: 1).frame(width: 310).offset(x: 138, y: -125)
                }.clipShape(RoundedRectangle(cornerRadius: 29))
            }
            .overlay(alignment: .bottomTrailing) { Text("TEST USD").font(.system(size: 7, weight: .medium, design: .monospaced)).tracking(1.2).foregroundStyle(.white.opacity(0.35)).padding(.trailing, 25).padding(.bottom, 8) }
    }
    private var nearbySection: some View {
        VStack(alignment: .leading, spacing: 19) {
            HStack { Text("Right here, right now").font(.system(size: 20, weight: .medium, design: .rounded)).tracking(-0.4); Spacer(); HStack(spacing: 4) { Circle().fill(model.bluetoothOn ? MeshTheme.green : MeshTheme.muted).frame(width: 5, height: 5); Text("\(model.nearby.count) nearby").font(.system(size: 10, design: .monospaced)) }.foregroundStyle(MeshTheme.muted) }
            if model.nearby.isEmpty {
                HStack(spacing: 15) {
                    Image(systemName: "dot.radiowaves.left.and.right").font(.system(size: 23, weight: .light)).frame(width: 50, height: 50).background(MeshTheme.soft, in: Circle())
                    VStack(alignment: .leading, spacing: 5) { Text("Bring a friend a little closer").font(.system(size: 14, weight: .medium)); Text(model.bluetoothStatus).font(.system(size: 11)).foregroundStyle(MeshTheme.muted) }
                }.padding(.vertical, 15)
            } else {
                HStack(alignment: .top, spacing: 0) {
                    ForEach(Array(model.nearby.prefix(4).enumerated()), id: \.element.id) { index, person in
                        Button { selectedPerson = person } label: {
                            VStack(spacing: 9) {
                                Avatar(name: person.profile.body.name, size: 62, index: index)
                                    .overlay(alignment: .bottomTrailing) { Circle().fill(MeshTheme.green).frame(width: 11, height: 11).overlay(Circle().stroke(MeshTheme.background, lineWidth: 3)).offset(x: -1, y: -1) }
                                Text(person.profile.body.name).font(.system(size: 12, weight: .medium)).lineLimit(1)
                                Text("Tap to pay").font(.system(size: 9)).foregroundStyle(MeshTheme.muted)
                            }.frame(maxWidth: .infinity)
                        }.buttonStyle(.plain).accessibilityIdentifier("nearby.\(person.profile.body.name)")
                    }
                    if model.nearby.count < 4 {
                        VStack(spacing: 9) {
                            Image(systemName: "antenna.radiowaves.left.and.right").font(.system(size: 20, weight: .ultraLight)).foregroundStyle(MeshTheme.muted).frame(width: 62, height: 62).overlay(Circle().stroke(MeshTheme.line, style: StrokeStyle(lineWidth: 1, dash: [3, 4])))
                            Text("Discovering").font(.system(size: 10)).foregroundStyle(MeshTheme.muted)
                        }.frame(maxWidth: .infinity)
                    }
                }
            }
            HStack(spacing: 7) { Image(systemName: "dot.radiowaves.left.and.right"); Text("Connected by Bluetooth. Kept between you.") }.font(.system(size: 10)).foregroundStyle(MeshTheme.muted).frame(maxWidth: .infinity).padding(.vertical, 11).background(MeshTheme.soft.opacity(0.75), in: RoundedRectangle(cornerRadius: 11))
        }
    }
    private var activitySection: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack { Text(tab == 0 ? "A little back & forth" : "Your activity").font(.system(size: 20, weight: .medium, design: .rounded)).tracking(-0.4); Spacer(); if tab == 0 && !model.activity.isEmpty { Button("See all") { tab = 1 }.font(.system(size: 11)).foregroundStyle(MeshTheme.muted) } }.padding(.bottom, 8)
            if model.activity.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Your next coffee starts here.").font(.system(size: 14, weight: .medium))
                    Text("Payments with friends will show up here.").font(.system(size: 12)).foregroundStyle(MeshTheme.muted)
                    if model.saved.snapshot?.balanceCents == 0 && !model.isBankBacked { Button("Add $125.40 in test money") { Task { await model.fundDemo() } }.font(.system(size: 12, weight: .semibold)).padding(.top, 7) }
                }.frame(maxWidth: .infinity, alignment: .leading).padding(20).background(.white.opacity(0.7), in: RoundedRectangle(cornerRadius: 20))
            } else {
                ForEach(Array(model.activity.prefix(tab == 0 ? 3 : 200).enumerated()), id: \.element.id) { index, item in
                    HStack(spacing: 13) {
                        if item.isBank { Image(systemName: "building.columns").font(.system(size: 18)).frame(width: 44, height: 44).background(MeshTheme.soft, in: Circle()) }
                        else { Avatar(name: item.name, size: 44, index: index) }
                        VStack(alignment: .leading, spacing: 4) { Text(item.name).font(.system(size: 14, weight: .medium)); Text(item.detail).font(.system(size: 11)).foregroundStyle(MeshTheme.muted).lineLimit(1) }
                        Spacer(minLength: 5)
                        VStack(alignment: .trailing, spacing: 4) {
                            Text("\(item.incoming ? "+" : "−")\(money(item.amount))").font(.system(size: 14, weight: .medium, design: .rounded)).foregroundStyle(item.incoming ? MeshTheme.green : MeshTheme.ink)
                            Text(activityStatus(item.status)).font(.system(size: 9, design: .monospaced)).foregroundStyle(item.status == "failed" ? .red : MeshTheme.muted)
                        }
                    }.padding(.vertical, 10)
                    if index < min(model.activity.count, tab == 0 ? 3 : 200) - 1 { Rectangle().fill(MeshTheme.line).frame(height: 1).padding(.leading, 57) }
                }
            }
        }
    }
    private func activityStatus(_ status: String) -> String {
        switch status { case "settled": return "Complete"; case "failed": return "Failed"; case "cancelled": return "Cancelled"; case "returned": return "Returned"; case "needs_review": return "In review"; default: return "Pending" }
    }
    private var privacyNote: some View {
        HStack(spacing: 5) { Image(systemName: "lock.shield"); Text("A small network. A little more human.") }.font(.system(size: 10)).foregroundStyle(MeshTheme.muted.opacity(0.8)).frame(maxWidth: .infinity).padding(.top, 1)
    }
    private func navButton(_ title: String, icon: String, index: Int) -> some View {
        Button { tab = index } label: { VStack(spacing: 5) { Image(systemName: icon).font(.system(size: 20, weight: .regular)); Text(title).font(.system(size: 10, weight: tab == index ? .semibold : .regular)) }.foregroundStyle(tab == index ? MeshTheme.green : MeshTheme.muted) }.accessibilityLabel(title)
    }
    private var nearbySheet: some View {
        VStack(alignment: .leading, spacing: 24) {
            HStack { Text("Who's nearby?").font(.system(size: 28, weight: .medium, design: .rounded)); Spacer(); CloseButton() }
            Text("Both of you just need Mesh open and Bluetooth on.").font(.system(size: 14)).foregroundStyle(MeshTheme.muted)
            if model.nearby.isEmpty { ContentUnavailableView("A little closer", systemImage: "dot.radiowaves.left.and.right", description: Text(model.bluetoothStatus)) }
            ForEach(Array(model.nearby.enumerated()), id: \.element.id) { index, person in
                Button { showNearby = false; DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { selectedPerson = person } } label: {
                    HStack(spacing: 14) { Avatar(name: person.profile.body.name, index: index); VStack(alignment: .leading, spacing: 5) { Text(person.profile.body.name).font(.system(size: 17, weight: .medium)); Text(person.fingerprint).font(MeshTheme.mono).foregroundStyle(MeshTheme.muted) }; Spacer(); Image(systemName: "arrow.up.right") }
                }.buttonStyle(.plain)
            }
            Spacer()
        }.padding(27).background(MeshTheme.background).foregroundStyle(MeshTheme.ink).presentationDetents([.medium, .large])
    }
}
