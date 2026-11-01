import SwiftUI

struct SendView: View {
    @EnvironmentObject var model: WalletModel
    @Environment(\.dismiss) private var dismiss
    let person: NearbyPerson
    @State private var amount = ""
    @State private var memo = ""
    @State private var sending = false
    @State private var receipt: LocalPayment?
    @State private var showIdentity = false
    var body: some View {
        VStack(spacing: 0) {
            if let receipt { result(receipt) }
            else {
                HStack {
                    Button { dismiss() } label: { Image(systemName: "arrow.left").font(.system(size: 18)).frame(width: 36, height: 36) }.accessibilityLabel("Back")
                    Spacer(); SectionLabel(text: "A LITTLE SOMETHING"); Spacer(); Color.clear.frame(width: 36, height: 36)
                }.padding(.top, 15)
                ScrollView(showsIndicators: false) {
                    VStack(spacing: 0) {
                        Avatar(name: person.profile.body.name, size: 76).padding(.top, 25)
                        Text("For \(person.profile.body.name)").font(.system(size: 24, weight: .medium, design: .rounded)).tracking(-0.6).padding(.top, 14)
                        Button { showIdentity.toggle() } label: { HStack(spacing: 5) { Image(systemName: "dot.radiowaves.left.and.right"); Text("Nearby · check identity"); Image(systemName: "chevron.down").font(.system(size: 8)) }.font(.system(size: 11)).foregroundStyle(MeshTheme.muted) }.padding(.top, 8)
                        if showIdentity {
                            VStack(spacing: 6) {
                                Text(person.fingerprint).font(.system(size: 16, weight: .medium, design: .monospaced))
                                Text("Compare this with the identity in your friend's settings. Names alone don't verify a person.").font(.system(size: 11)).foregroundStyle(MeshTheme.muted).multilineTextAlignment(.center)
                            }.padding(15).background(MeshTheme.soft, in: RoundedRectangle(cornerRadius: 15)).padding(.top, 13)
                        }
                        HStack(alignment: .firstTextBaseline, spacing: 3) {
                            Text("$").font(.system(size: 37, weight: .light, design: .rounded)).foregroundStyle(MeshTheme.muted)
                            Text(amount.isEmpty ? "0" : amount).font(.system(size: 76, weight: .regular, design: .rounded)).tracking(-3).minimumScaleFactor(0.45).lineLimit(1).contentTransition(.numericText())
                        }.padding(.top, 28).frame(height: 125).accessibilityElement(children: .combine).accessibilityIdentifier("payment.amount")
                        Text("\(money(model.availableCents)) available · test money").font(.system(size: 11)).foregroundStyle(MeshTheme.muted)
                        TextField("What's it for?", text: $memo).font(.system(size: 14)).multilineTextAlignment(.center).padding(15).background(.white.opacity(0.8), in: Capsule()).padding(.horizontal, 42).padding(.top, 24).onChange(of: memo) { _, value in if value.count > 80 { memo = String(value.prefix(80)) } }.accessibilityIdentifier("payment.memo")
                        AmountKeypad(amount: $amount).padding(.top, 23)
                    }
                }
                Button {
                    sending = true
                    Task { receipt = await model.send(to: person, amount: amount, memo: memo); sending = false }
                } label: { HStack(spacing: 8) { Image(systemName: "\(sending ? "hourglass" : "arrow.up.right")"); Text(sending ? "Sending…" : "Pay \(person.profile.body.name)") } }
                .buttonStyle(PrimaryButton()).disabled(amount.isEmpty || amount == "0" || sending || model.syncing).opacity(amount.isEmpty ? 0.5 : 1).padding(.top, 10).accessibilityIdentifier("payment.confirm")
                HStack(spacing: 4) { Image(systemName: "lock"); Text("Encrypted nearby. Confirmed when connected.") }.font(.system(size: 9)).foregroundStyle(MeshTheme.muted).padding(.top, 14).padding(.bottom, 12)
            }
        }.padding(.horizontal, 26).background(MeshTheme.background).foregroundStyle(MeshTheme.ink).presentationDragIndicator(.visible)
    }
    private func result(_ local: LocalPayment) -> some View {
        let settled = local.state == "settled"
        let failed = local.state == "failed"
        return VStack(spacing: 0) {
            HStack { Spacer(); CloseButton() }.padding(.top, 18)
            Spacer()
            ZStack {
                Circle().stroke(MeshTheme.green.opacity(0.08), lineWidth: 1).frame(width: 210)
                Circle().stroke(MeshTheme.green.opacity(0.08), lineWidth: 1).frame(width: 165)
                Circle().fill(MeshTheme.lime).frame(width: 120)
                Image(systemName: failed ? "exclamationmark" : (settled ? "checkmark" : "arrow.up.right")).font(.system(size: 40, weight: .light))
            }.frame(height: 230)
            Text(failed ? "Couldn't go through." : (settled ? "Good things, sent." : "On its way.")).font(.system(size: 35, weight: .medium, design: .rounded)).tracking(-1.2).padding(.top, 24)
            Text("\(money(local.payment.body.amountCents)) to \(person.profile.body.name)").font(.system(size: 18)).foregroundStyle(MeshTheme.muted).padding(.top, 13)
            Text(failed ? (local.failure ?? "Your balance hasn't been charged.") : (settled ? "All settled. A little less back and forth." : "Saved on your iPhone. It stays pending until the balance is checked online, and may expire or fail."))
                .font(.system(size: 14)).foregroundStyle(MeshTheme.muted).multilineTextAlignment(.center).lineSpacing(5).padding(.horizontal, 15).padding(.top, 22)
            Spacer()
            Button("Back to nearby") { dismiss() }.buttonStyle(PrimaryButton()).padding(.bottom, 30)
        }
    }
}

struct AmountKeypad: View {
    @Binding var amount: String
    private let keys = [["1", "2", "3"], ["4", "5", "6"], ["7", "8", "9"], [".", "0", "⌫"]]
    var body: some View {
        VStack(spacing: 0) {
            ForEach(keys, id: \.self) { row in
                HStack(spacing: 0) {
                    ForEach(row, id: \.self) { key in
                        Button { enter(key) } label: {
                            Group { if key == "⌫" { Image(systemName: "delete.left").font(.system(size: 21, weight: .light)) } else { Text(key).font(.system(size: 27, weight: .regular, design: .rounded)) } }
                                .frame(maxWidth: .infinity).frame(height: 59).contentShape(Rectangle())
                        }.buttonStyle(.plain).accessibilityLabel(key == "⌫" ? "Delete digit" : key).accessibilityIdentifier("keypad.\(key)")
                    }
                }
            }
        }
    }
    private func enter(_ key: String) {
        UISelectionFeedbackGenerator().selectionChanged()
        if key == "⌫" { if !amount.isEmpty { amount.removeLast() }; return }
        if key == "." { if !amount.contains(".") { amount = amount.isEmpty ? "0." : amount + "." }; return }
        if let dot = amount.firstIndex(of: "."), amount.distance(from: dot, to: amount.endIndex) > 2 { return }
        guard amount.count < 7 else { return }
        if amount == "0" { amount = key } else { amount += key }
    }
}
