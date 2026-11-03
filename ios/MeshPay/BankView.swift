import SwiftUI
import LinkKit

struct BankView: View {
    @EnvironmentObject var model: WalletModel
    @State private var amount = ""
    @State private var busy = false
    @State private var linkToken: String?
    @State private var complete = false
    @State private var adding = false
    private var banking: BankingStatus? { model.saved.snapshot?.banking }
    private var bankPending: Bool { model.isBankBacked && !["not_started", "ready", "relink_required"].contains(banking?.bankStatus ?? "not_started") }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                HStack { SectionLabel(text: model.isBankBacked ? "YOUR BANK & WALLET" : "BACK TO YOUR BANK"); Spacer(); CloseButton() }.padding(.top, 20)
                Image(systemName: "building.columns").font(.system(size: 31, weight: .light)).frame(width: 80, height: 80).background(MeshTheme.lime, in: RoundedRectangle(cornerRadius: 27)).padding(.top, 37)
                Text(complete ? (adding ? "A little on its way." : "Heading home.") : "Your money.\nYour bank.").font(.system(size: 39, weight: .medium, design: .rounded)).tracking(-1.4).padding(.top, 26)
                Text(complete ? "Your test transfer is processing. Follow it in Activity." : "For the things that happen a little further away.").font(.system(size: 15)).lineSpacing(5).foregroundStyle(MeshTheme.muted).padding(.top, 12)
                if model.bankLinkInterrupted || banking?.bankStatus == "relink_required" {
                    Label(model.bankLinkInterrupted ? "Bank sign-in was interrupted. Start again below to reconnect securely." : "Your bank sign-in expired. Connect with Plaid again below.", systemImage: "arrow.clockwise")
                        .font(.system(size: 13)).lineSpacing(4).padding(18).background(MeshTheme.soft, in: RoundedRectangle(cornerRadius: 18)).padding(.top, 24).accessibilityIdentifier("bank.reconnect")
                }
                if model.saved.bankLinkAttempt != nil {
                    Label("Your bank connection is saved. Reconnect to the internet to finish syncing.", systemImage: "icloud.and.arrow.up").font(.system(size: 13)).padding(.top, 22)
                }
                if model.isBankBacked, let banking, !banking.blocked.isEmpty {
                    Label(banking.debtCents > 0 ? "A bank deposit was returned. Add \(money(banking.debtCents)) to restore your wallet before spending." : "Your wallet needs a bank review. Transfers stay visible in Activity.", systemImage: "exclamationmark.circle")
                        .font(.system(size: 13)).lineSpacing(4).padding(18).background(MeshTheme.soft, in: RoundedRectangle(cornerRadius: 18)).padding(.top, 24)
                }
                if model.isBankBacked && banking?.status != "verified" {
                    verificationCard
                } else if let bank = model.saved.snapshot?.bank {
                    linkedBank(bank)
                } else if bankPending {
                    VStack(alignment: .leading, spacing: 14) {
                        Label(banking?.bankStatus == "needs_review" ? "Bank connection needs attention" : "Checking your bank connection", systemImage: "clock")
                        Text("Your connection is saved. You can close this screen while the bank responds.").font(.system(size: 13)).lineSpacing(4).foregroundStyle(MeshTheme.muted)
                    }.padding(23).frame(maxWidth: .infinity, alignment: .leading).background(.white, in: RoundedRectangle(cornerRadius: 25)).padding(.top, 32)
                } else {
                    connectionCard
                }
                HStack(alignment: .top, spacing: 9) {
                    Image(systemName: "info.circle")
                    Text(model.isBankBacked ? "Sandbox only. Bank transfers are handled by Dwolla Sandbox. Test deposits become available after confirmation; no real money moves." : "This is a sandbox. Cash outs are simulated and no money reaches a real bank account.").lineSpacing(4)
                }.font(.system(size: 11)).foregroundStyle(MeshTheme.muted).padding(.top, 28).padding(.bottom, 28)
            }.padding(.horizontal, 27)
        }.background(MeshTheme.background).foregroundStyle(MeshTheme.ink).presentationDragIndicator(.visible)
        .sheet(isPresented: Binding(get: { linkToken != nil }, set: { if !$0 { linkToken = nil } }), onDismiss: { model.cancelBankSignIn() }) {
            if let linkToken { PlaidLinkView(token: linkToken, onComplete: { token, account in self.linkToken = nil; Task { await model.completeLink(publicToken: token, accountID: account) } }, onExit: { self.linkToken = nil }, onError: { model.error = $0; self.linkToken = nil }) }
        }
    }
    private var verificationCard: some View {
        VStack(alignment: .leading, spacing: 17) {
            Label("Start with a test wallet", systemImage: "person.crop.circle.badge.checkmark").font(.system(size: 16, weight: .medium))
            Text("Create a synthetic identity with Dwolla Sandbox, then connect a test bank with Plaid. No personal identity details are needed.").font(.system(size: 13)).lineSpacing(4).foregroundStyle(MeshTheme.muted)
            if banking?.status == "not_started" {
                Button { busy = true; Task { await model.createTestWallet(); busy = false } } label: { Text(busy ? "Creating…" : "Create my test wallet") }.buttonStyle(PrimaryButton()).disabled(busy || !model.online || model.isPreview).accessibilityIdentifier("bank.verify")
            } else {
                Label(banking?.blocked.isEmpty == false ? "Verification needs attention" : "Verification in progress", systemImage: "clock").font(.system(size: 14))
            }
        }.padding(23).background(.white, in: RoundedRectangle(cornerRadius: 25)).padding(.top, 32)
    }
    private func linkedBank(_ bank: Bank) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 13) {
                Image(systemName: "checkmark.shield").font(.system(size: 22))
                VStack(alignment: .leading, spacing: 5) { Text(bank.label).font(.system(size: 15, weight: .medium)); Text("•••• \(bank.mask) · \(bank.mode == "demo" ? "Demo account" : "Sandbox account")").font(.system(size: 12)).foregroundStyle(MeshTheme.muted) }
                Spacer(); Image(systemName: "checkmark.circle.fill").foregroundStyle(MeshTheme.green)
            }.padding(20).background(.white, in: RoundedRectangle(cornerRadius: 22)).padding(.top, 32)
            if !complete {
                if model.isBankBacked {
                    Picker("Transfer", selection: $adding) { Text("Cash out").tag(false); Text("Add money").tag(true) }.pickerStyle(.segmented).padding(.top, 25)
                }
                Text("How much?").font(.system(size: 13)).foregroundStyle(MeshTheme.muted).padding(.top, 29)
                HStack(alignment: .firstTextBaseline) { Text("$").font(.system(size: 30, weight: .light)); TextField("0.00", text: $amount).font(.system(size: 50, weight: .regular, design: .rounded)).keyboardType(.decimalPad).accessibilityIdentifier("bank.amount") }.padding(.vertical, 14)
                Rectangle().fill(MeshTheme.line).frame(height: 1)
                Text(adding ? "One-time debit from your linked test bank" : "\(money(model.availableCents)) available").font(.system(size: 12)).foregroundStyle(MeshTheme.muted).padding(.top, 12)
                Button { busy = true; Task { complete = adding ? await model.addFromBank(amount: amount) : await model.cashOut(amount: amount); busy = false } } label: { Text(busy ? "Confirming…" : (adding ? "Authorize test deposit" : "Cash out test money")) }.buttonStyle(PrimaryButton()).disabled(amount.isEmpty || busy || !model.online || model.isPreview).padding(.top, 35)
            }
        }
    }
    private var connectionCard: some View {
        VStack(alignment: .leading, spacing: 17) {
            Label("Connect securely with Plaid", systemImage: "lock.shield").font(.system(size: 15, weight: .medium))
            Text("Bank credentials stay with your bank and Plaid.").font(.system(size: 12)).foregroundStyle(MeshTheme.muted)
            Button { busy = true; Task { do { linkToken = try await model.linkToken() } catch { model.error = error.localizedDescription }; busy = false } } label: { HStack { Text(busy ? "Connecting…" : "Connect with Plaid"); Spacer(); Image(systemName: "arrow.up.right") } }.buttonStyle(PrimaryButton()).disabled(busy || model.saved.snapshot?.plaidConfigured != true || !model.online || model.isPreview || model.saved.bankLinkAttempt != nil).opacity(model.saved.snapshot?.plaidConfigured == true ? 1 : 0.45).accessibilityIdentifier("bank.plaid")
            if model.saved.snapshot?.plaidConfigured != true { Text(model.isBankBacked ? "Bank connection is unavailable. Ask the person running this test to finish the Plaid Sandbox setup." : "Plaid Sandbox isn't connected yet. You can try the full flow with a demo bank.").font(.system(size: 12)).foregroundStyle(MeshTheme.muted).lineSpacing(4) }
            if !model.isBankBacked { Button { Task { await model.useDemoBank() } } label: { Text("Try with a demo bank").font(.system(size: 14, weight: .medium)).frame(maxWidth: .infinity).padding(12) }.disabled(!model.online || model.isPreview).accessibilityIdentifier("bank.demo") }
        }.padding(23).background(.white, in: RoundedRectangle(cornerRadius: 25)).padding(.top, 32)
    }
}

struct PlaidLinkView: View {
    let token: String
    let onComplete: (String, String) -> Void
    let onExit: () -> Void
    let onError: (String) -> Void
    @State private var session: PlaidLinkSession?
    @State private var presenting = false
    @State private var accounts: [(id: String, name: String)] = []
    @State private var publicToken: String?
    var body: some View {
        VStack(spacing: 25) {
            if let publicToken {
                Text("Choose your account").font(.system(size: 27, weight: .medium, design: .rounded))
                ForEach(accounts, id: \.id) { account in Button(account.name) { onComplete(publicToken, account.id) }.buttonStyle(PrimaryButton()) }
                Button("Cancel", action: onExit)
            } else { ProgressView("Opening Plaid…") }
        }.padding(27).frame(maxWidth: .infinity, maxHeight: .infinity).background(MeshTheme.background)
            .task { createSession() }
            .sheet(isPresented: $presenting) { session?.sheet() }
    }
    private func createSession() {
        guard session == nil else { return }
        do {
            let configuration = LinkTokenConfiguration(token: token, onSuccess: { success in
                DispatchQueue.main.async {
                    accounts = success.metadata.accounts.map { ($0.id, "\($0.name) · \($0.mask ?? "••••")") }
                    presenting = false
                    if accounts.count == 1 { onComplete(success.publicToken, accounts[0].id) }
                    else if accounts.isEmpty { onError("No eligible bank account was selected.") }
                    else { publicToken = success.publicToken }
                }
            }, onExit: { exit in
                DispatchQueue.main.async { presenting = false; if let error = exit.error { onError(error.localizedDescription) } else { onExit() } }
            }, onEvent: nil, onLoad: { DispatchQueue.main.async { presenting = true } })
            session = try Plaid.createPlaidLinkSession(configuration: configuration)
        } catch { onError(error.localizedDescription) }
    }
}
