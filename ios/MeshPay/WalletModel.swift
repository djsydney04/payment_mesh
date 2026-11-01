import Foundation
import SwiftUI
import Network
import LocalAuthentication

@MainActor
final class WalletModel: ObservableObject {
    @Published private(set) var saved: SavedWallet
    @Published private(set) var profile: ProfileResult
    @Published var nearby: [NearbyPerson] = []
    @Published var bluetoothStatus = "Starting Bluetooth"
    @Published var bluetoothOn = false
    @Published var syncing = false
    @Published var online = false
    @Published var error: String?
    @Published var toast: String?
    @Published private(set) var bankLinkInterrupted = false
    private let core: RustCore
    private let disk: WalletDisk
    private var mesh: BluetoothMesh?
    private let network = NWPathMonitor()
    private var syncTimer: Timer?

    init() throws {
        let engine = try RustCore()
        let storage = try WalletDisk()
        let initial = try storage.load()
        core = engine; disk = storage; saved = initial
        profile = try engine.call(["op": "profile", "name": initial.name.isEmpty ? "You" : initial.name])
        bankLinkInterrupted = initial.bankLinkInProgress == true && initial.bankLinkAttempt == nil
        if ProcessInfo.processInfo.arguments.contains("--preview") {
            try loadPreview(); return
        }
        network.pathUpdateHandler = { [weak self] _ in Task { @MainActor in await self?.sync() } }
        network.start(queue: DispatchQueue(label: "app.mesh.network"))
        if saved.onboarded { startBluetooth() }
        syncTimer = Timer.scheduledTimer(withTimeInterval: 15, repeats: true) { [weak self] _ in Task { @MainActor in await self?.sync() } }
    }
    var isPreview: Bool { ProcessInfo.processInfo.arguments.contains("--preview") }
    var publicKey: String { profile.publicKey }
    var pending: [LocalPayment] { saved.journal.filter { $0.state == "pending" } }
    var isBankBacked: Bool { saved.snapshot?.bankRail == "dwolla_sandbox" }
    var bankReady: Bool { !isBankBacked || (saved.snapshot?.banking?.status == "verified" && saved.snapshot?.banking?.blocked == "") }
    var availableCents: Int64 {
        guard bankReady else { return 0 }
        let reserved = Set(saved.snapshot?.reservedPaymentIds ?? [])
        let payments = pending.filter { !reserved.contains($0.id) }.map(\.payment)
        guard let object = try? Wire.object(payments), let result = try? core.call(["op": "spendable", "balance_cents": saved.snapshot?.balanceCents ?? 0, "pending": object], as: SpendableResult.self) else { return 0 }
        let uncertainCashout = saved.cashout.flatMap { attempt in saved.snapshot?.withdrawals.contains(where: { $0.id == attempt.id }) == true ? nil : attempt.amountCents } ?? 0
        return max(0, result.amountCents - uncertainCashout)
    }
    var activity: [ActivityItem] {
        var items = (saved.snapshot?.payments ?? []).map { p in
            let incoming = p.recipient == publicKey
            return ActivityItem(id: p.id, name: incoming ? p.senderName : p.recipientName, detail: p.memo.isEmpty ? (incoming ? "Received nearby" : "Paid nearby") : p.memo, amount: p.amountCents, incoming: incoming, status: p.status, at: p.settledAt, isBank: false)
        }
        let settledIDs = Set(items.map(\.id))
        items += saved.journal.filter { !settledIDs.contains($0.id) && $0.state != "settled" }.map { p in
            ActivityItem(id: p.id, name: p.friendName, detail: p.failure ?? (p.state == "pending" ? "Waiting to sync" : "Payment failed"), amount: p.payment.body.amountCents, incoming: p.payment.body.recipient == publicKey, status: p.state, at: p.payment.body.createdAt, isBank: false)
        }
        items += (saved.snapshot?.withdrawals ?? []).map { w in ActivityItem(id: w.id, name: w.bankLabel, detail: w.status == "settled" ? (isBankBacked ? "Cash out complete" : "Demo cash out complete") : "Cash out processing", amount: w.amountCents, incoming: false, status: w.status, at: w.createdAt, isBank: true) }
        items += (saved.snapshot?.fundings ?? []).map { f in ActivityItem(id: f.id, name: f.bankLabel, detail: f.status == "settled" ? "Added to your wallet" : "Bank deposit · \(f.status.replacingOccurrences(of: "_", with: " "))", amount: f.amountCents, incoming: true, status: f.status, at: f.createdAt, isBank: true) }
        return items.sorted { $0.at > $1.at }
    }
    func finishOnboarding(name: String, server: String) async {
        do {
            let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
            let next: ProfileResult = try core.call(["op": "profile", "name": name])
            _ = try validatedURL(server)
            var update = saved
            update.name = name; update.serverUrl = server.trimmingCharacters(in: .whitespacesAndNewlines); update.onboarded = true
            try commit(update); profile = next
            startBluetooth()
            await sync()
        } catch { self.error = error.localizedDescription }
    }
    func updateServer(_ url: String) async {
        guard pending.isEmpty, saved.cashout == nil, saved.funding == nil, saved.bankLinkAttempt == nil, saved.bankLinkInProgress != true else { error = "Sync your pending payments before changing servers."; return }
        do {
            _ = try validatedURL(url)
            var next = saved; next.serverUrl = url; next.snapshot = nil; next.enrollmentId = nil
            try commit(next); online = false
            await sync()
        } catch { self.error = error.localizedDescription }
    }
    func fundDemo() async {
        do { let _: EmptyResponse = try await rpc("demo_fund"); await sync(); toast = "Test money added. You're ready to try Mesh." }
        catch { self.error = error.localizedDescription }
    }
    func send(to person: NearbyPerson, amount: String, memo: String) async -> LocalPayment? {
        do {
            guard !isPreview else { throw MeshError.message("This is a design preview. Open the app normally to make a test payment.") }
            guard bankReady else { throw MeshError.message("Finish wallet verification in Bank before paying nearby.") }
            let result: CreatedPayment = try core.call(["op": "create_payment", "ledger_id": saved.snapshot?.ledgerId ?? "", "recipient": Wire.object(person.profile), "amount": amount, "memo": memo, "balance_cents": availableCents, "pending": []])
            try await authenticate("Confirm \(money(result.payment.body.amountCents)) to \(person.profile.body.name)")
            // Recheck after the system prompt: another operation may have spent the balance.
            guard result.payment.body.amountCents <= availableCents else { throw MeshError.message("Your available balance changed. Try a smaller amount.") }
            let local = LocalPayment(payment: result.payment, packet: result.packet, friendName: person.profile.body.name, state: "pending")
            var next = saved; next.journal.append(local)
            try commit(next) // Durable before any radio or HTTP side effect.
            mesh?.broadcast(result.packet)
            await sync()
            return saved.journal.first { $0.id == local.id } ?? local
        } catch { self.error = error.localizedDescription; return nil }
    }
    func cashOut(amount: String) async -> Bool {
        do {
            guard saved.cashout == nil else { throw MeshError.message("Your previous cash out is still syncing. Reconnect to check it.") }
            guard pending.isEmpty else { throw MeshError.message("Sync your nearby payments before cashing out.") }
            guard online else { throw MeshError.message("Connect to the internet to cash out.") }
            // Let Rust validate the amount with the same currency and cent rules.
            let result: SpendableResult = try parseAmount(amount)
            guard result.amountCents <= availableCents else { throw MeshError.message("Your available balance is too low.") }
            try await authenticate("Cash out \(money(result.amountCents)) to your linked bank")
            guard result.amountCents <= availableCents, pending.isEmpty, saved.cashout == nil else { throw MeshError.message("Your balance changed. Check it and try again.") }
            var next = saved; next.cashout = CashoutAttempt(id: UUID().uuidString.lowercased(), amountCents: result.amountCents)
            let attemptID = next.cashout!.id
            try commit(next)
            await sync()
            if saved.cashout != nil { throw MeshError.message("Cash out saved. Reconnect to confirm its status; don't start another one.") }
            return saved.snapshot?.withdrawals.contains(where: { $0.id == attemptID }) == true
        } catch { self.error = error.localizedDescription; return false }
    }
    func useDemoBank() async {
        do { let _: EmptyResponse = try await rpc("demo_link_bank"); await sync(); toast = "Demo checking connected" }
        catch { self.error = error.localizedDescription }
    }
    func linkToken() async throws -> String {
        guard !isPreview else { throw MeshError.message("Open the app normally to connect a test bank.") }
        guard saved.bankLinkAttempt == nil else { throw MeshError.message("Your bank connection is saved. Reconnect to finish syncing it.") }
        var next = saved; next.bankLinkInProgress = true; try commit(next)
        bankLinkInterrupted = false
        do { let value: LinkToken = try await rpc("link_token"); return value.linkToken }
        catch { cancelBankSignIn(); throw error }
    }
    func cancelBankSignIn() {
        do { var next = saved; next.bankLinkInProgress = false; try commit(next); bankLinkInterrupted = false }
        catch { self.error = error.localizedDescription }
    }
    func completeLink(publicToken: String, accountID: String) async {
        do {
            guard !isPreview else { throw MeshError.message("Open the app normally to connect a test bank.") }
            guard saved.bankLinkAttempt == nil else { throw MeshError.message("Your previous bank connection is still syncing.") }
            self.error = nil
            var next = saved
            next.bankLinkAttempt = BankLinkAttempt(id: UUID().uuidString.lowercased(), publicToken: publicToken, accountId: accountID)
            next.bankLinkInProgress = false
            try commit(next) // Save the intent before its first network request.
            bankLinkInterrupted = false
            await sync()
            guard self.error == nil else { return }
            toast = saved.bankLinkAttempt == nil ? (isBankBacked ? "Bank connection saved · verification pending" : "Bank connected with Plaid Sandbox") : "Connection saved. Reconnect to finish syncing."
        } catch { self.error = error.localizedDescription }
    }
    func createTestWallet() async {
        do {
            guard isBankBacked, !isPreview else { throw MeshError.message("Connect to the bank-backed Sandbox server first.") }
            var next = saved
            if next.enrollmentId == nil { next.enrollmentId = UUID().uuidString.lowercased(); try commit(next) }
            // Synthetic fixture only; no personal identity information is collected or stored on the phone.
            let profile: [String: String] = ["firstName": "John", "lastName": "Doe", "email": "mesh-\(publicKey.prefix(20))@example.com", "address1": "99-99 33rd St", "city": "Some City", "state": "NY", "postalCode": "11101", "dateOfBirth": "1970-01-01", "ssn": "1234"]
            let _: Receipt = try await rpc("bank_enroll", payload: ["id": saved.enrollmentId!, "profile": profile, "synthetic_test_data": true])
            await sync(); toast = "Test wallet created · verification pending"
        } catch { self.error = error.localizedDescription }
    }
    func addFromBank(amount: String) async -> Bool {
        do {
            guard isBankBacked, online, !isPreview else { throw MeshError.message("Connect to the internet to add test money from your bank.") }
            guard saved.funding == nil else { throw MeshError.message("Your previous deposit is still syncing. Check Activity before starting another.") }
            let result = try parseAmount(amount)
            try await authenticate("Authorize a one-time test debit of \(money(result.amountCents)) from your linked Sandbox bank")
            guard saved.funding == nil else { throw MeshError.message("Your previous deposit is still syncing.") }
            let attempt = CashoutAttempt(id: UUID().uuidString.lowercased(), amountCents: result.amountCents)
            var next = saved; next.funding = attempt; try commit(next)
            await sync()
            if saved.funding != nil { throw MeshError.message("Deposit saved. Reconnect to confirm its status; don't start another one.") }
            return saved.snapshot?.fundings?.contains(where: { $0.id == attempt.id }) == true
        } catch { self.error = error.localizedDescription; return false }
    }
    private func reconcileJournal(_ wallet: inout SavedWallet, snapshot: WalletSnapshot) {
        let states = Dictionary(uniqueKeysWithValues: snapshot.payments.map { ($0.id, $0.status) })
        for i in wallet.journal.indices {
            guard let status = states[wallet.journal[i].id] else { continue }
            if status == "settled" { wallet.journal[i].state = "settled"; wallet.journal[i].failure = nil }
            else if ["failed", "cancelled", "returned"].contains(status) { wallet.journal[i].state = "failed"; wallet.journal[i].failure = "Payment \(status). Check Activity before trying again." }
        }
    }
    func sync() async {
        guard saved.onboarded, !syncing, !isPreview else { return }
        syncing = true; defer { syncing = false }
        do {
            profile = try core.call(["op": "profile", "name": saved.name])
            mesh?.updateProfile(profile.profile)
            let _: EmptyResponse = try await rpc("register", payload: Wire.object(profile.profile))
            let snapshot: WalletSnapshot = try await rpc("snapshot")
            var next = saved; next.snapshot = snapshot
            reconcileJournal(&next, snapshot: snapshot)
            try commit(next)
            if let attempt = saved.bankLinkAttempt {
                do {
                    let receipt: BankLinkReceipt = try await rpc("link_bank", payload: ["id": attempt.id, "public_token": attempt.publicToken, "account_id": attempt.accountId])
                    guard !isBankBacked || receipt.id == attempt.id else { throw MeshError.message("Couldn't confirm this bank connection. It stays saved for retry.") }
                    if receipt.status == "relink_required" { throw MeshError.api("plaid_relink_required", "Your bank sign-in expired. Connect with Plaid again.") }
                    let refreshed: WalletSnapshot = try await rpc("snapshot")
                    var next = saved; next.snapshot = refreshed; next.bankLinkAttempt = nil; next.bankLinkInProgress = false
                    try commit(next)
                } catch let e as MeshError where ["plaid_relink_required", "invalid_bank_token", "invalid_bank_account", "id_conflict", "setup_exists", "verification_required"].contains(e.code ?? "") {
                    let refreshed: WalletSnapshot = try await rpc("snapshot")
                    var next = saved; next.snapshot = refreshed; next.bankLinkAttempt = nil; next.bankLinkInProgress = false
                    try commit(next); self.error = e.localizedDescription
                }
            }
            var confirmed = Set<String>()
            for item in pending {
                do {
                    let receipt: Receipt = try await rpc("settle", payload: Wire.object(item.payment))
                    if receipt.status == "settled" { confirmed.insert(item.id) }
                } catch let e as MeshError where ["insufficient_funds", "invalid_payment", "id_conflict", "wrong_ledger"].contains(e.code ?? "") {
                    try setState(item.id, state: "failed", failure: e.localizedDescription)
                }
            }
            if let attempt = saved.funding {
                do {
                    let receipt: Receipt = try await rpc("bank_fund", payload: ["id": attempt.id, "amount_cents": attempt.amountCents, "authorize_test_debit": true])
                    let refreshed: WalletSnapshot = try await rpc("snapshot")
                    var next = saved; next.snapshot = refreshed
                    if receipt.id == attempt.id { next.funding = nil }
                    try commit(next)
                } catch let e as MeshError where ["authorization_required", "bank_required", "invalid_payment", "id_conflict", "wallet_review", "verification_required"].contains(e.code ?? "") {
                    var next = saved; next.funding = nil; try commit(next); self.error = e.localizedDescription
                }
            }
            if let attempt = saved.cashout {
                do {
                    let _: Receipt = try await rpc("withdraw", payload: ["id": attempt.id, "amount_cents": attempt.amountCents])
                    // Refresh before releasing the local reservation.
                    let refreshed: WalletSnapshot = try await rpc("snapshot")
                    var next = saved
                    next.snapshot = refreshed
                    next.cashout = nil; try commit(next)
                } catch let e as MeshError where ["insufficient_funds", "bank_required", "invalid_payment", "id_conflict", "wallet_review", "verification_required"].contains(e.code ?? "") {
                    var next = saved; next.cashout = nil; try commit(next); self.error = e.localizedDescription
                }
            }
            let refreshed: WalletSnapshot = try await rpc("snapshot")
            var final = saved; final.snapshot = refreshed
            reconcileJournal(&final, snapshot: refreshed)
            for i in final.journal.indices where confirmed.contains(final.journal[i].id) { final.journal[i].state = "settled" }
            try commit(final)
            online = true
        } catch { online = false }
    }
    func rebroadcast() {
        for item in pending where item.payment.body.sender == publicKey { mesh?.broadcast(item.packet) }
    }
    private func receive(_ payment: Payment, packet: MeshPacket) {
        guard !saved.journal.contains(where: { $0.id == payment.body.id }) else { return }
        let name = nearby.first { $0.id == payment.body.sender }?.profile.body.name ?? "Nearby friend"
        var next = saved
        guard next.journal.filter({ $0.state == "pending" }).count < 512 else { return }
        next.journal.append(LocalPayment(payment: payment, packet: packet, friendName: name, state: "pending"))
        do { try commit(next); toast = "Payment received · waiting to sync"; Task { await sync() } }
        catch { self.error = "Couldn't save the incoming payment. Keep both phones nearby and try again." }
    }
    private func setState(_ id: String, state: String, failure: String? = nil) throws {
        var next = saved
        if let i = next.journal.firstIndex(where: { $0.id == id }) { next.journal[i].state = state; next.journal[i].failure = failure }
        try commit(next)
    }
    private func commit(_ value: SavedWallet) throws { if !isPreview { try disk.save(value) }; saved = value }
    private func parseAmount(_ amount: String) throws -> SpendableResult {
        try core.call(["op": "parse_amount", "amount": amount])
    }
    private func authenticate(_ reason: String) async throws {
        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &error) else { throw MeshError.message("Set a device passcode to confirm payments securely.") }
        guard try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason) else { throw MeshError.message("Payment wasn't confirmed.") }
    }
    private func validatedURL(_ value: String) throws -> URL {
        guard let url = URL(string: value.trimmingCharacters(in: .whitespacesAndNewlines)), let scheme = url.scheme, let host = url.host, url.user == nil, url.password == nil, url.query == nil, url.fragment == nil, url.path.isEmpty || url.path == "/" else { throw MeshError.message("Enter your server address, such as https://mesh.example.com") }
        #if DEBUG
        guard scheme == "https" || (scheme == "http" && (host == "localhost" || host == "127.0.0.1" || host.hasSuffix(".local") || host.hasPrefix("192.168.") || host.hasPrefix("10.") || host.hasPrefix("172."))) else { throw MeshError.message("Use HTTPS, or a local development server.") }
        #else
        guard scheme == "https" else { throw MeshError.message("Your server must use HTTPS.") }
        #endif
        return url
    }
    private func rpc<T: Decodable>(_ action: String, payload: Any = [String: String]()) async throws -> T {
        guard !isPreview else { throw MeshError.message("This preview cannot contact your payment server.") }
        let base = try validatedURL(saved.serverUrl)
        var request = URLRequest(url: base.appendingPathComponent("v1/rpc"))
        request.httpMethod = "POST"; request.timeoutInterval = 25
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try core.request(action: action, payload: payload)
        let (data, response) = try await URLSession.shared.data(for: request)
        guard data.count <= 1_048_576, let response = response as? HTTPURLResponse else { throw MeshError.message("Your server returned an unreadable response.") }
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw MeshError.message("Your server returned an unreadable response.") }
        guard (200..<300).contains(response.statusCode), object["ok"] as? Bool == true, let value = object["data"] else {
            let apiError = object["error"] as? [String: Any]
            throw MeshError.api(apiError?["code"] as? String ?? "request_failed", apiError?["message"] as? String ?? "Your server couldn't complete this request.")
        }
        return try Wire.decoder.decode(T.self, from: JSONSerialization.data(withJSONObject: value))
    }
    private func startBluetooth() {
        guard mesh == nil else { return }
        let mesh = BluetoothMesh(core: core, profile: profile.profile)
        self.mesh = mesh
        mesh.$people.assign(to: &$nearby)
        mesh.$status.assign(to: &$bluetoothStatus)
        mesh.$poweredOn.assign(to: &$bluetoothOn)
        mesh.onPayment = { [weak self] payment, packet in self?.receive(payment, packet: packet) }
        mesh.onConnected = { [weak self] in self?.rebroadcast() }
        mesh.start()
    }
    private func loadPreview() throws {
        saved.name = "Dylan"; saved.onboarded = true
        profile = try core.call(["op": "profile", "name": "Dylan"])
        // Explicit UI preview fixtures never enter the HTTP ledger or Bluetooth transport.
        let data = Data(previewSnapshot(publicKey: publicKey).utf8)
        saved.snapshot = try Wire.decoder.decode(WalletSnapshot.self, from: data)
        let names = ["Sam", "Alex", "Jordan"]
        nearby = try names.enumerated().map { index, name in
            let seed = String(repeating: String(index + 1), count: 64)
            let object = seed.withCString { mesh_engine_new($0) }!
            defer { mesh_engine_free(object) }
            let command = "{\"op\":\"profile\",\"name\":\"\(name)\"}"
            let output = command.withCString { mesh_engine_call(object, $0) }!
            defer { mesh_string_free(output) }
            struct Response: Decodable { let data: ProfileResult }
            let value = try Wire.decoder.decode(Response.self, from: Data(String(cString: output).utf8)).data
            return NearbyPerson(profile: value.profile, fingerprint: value.fingerprint, lastSeen: Date(), signal: -45 - index * 10)
        }
        if ProcessInfo.processInfo.arguments.contains("--preview-bank") {
            var snapshot = try JSONSerialization.jsonObject(with: data) as! [String: Any]
            snapshot["bank_rail"] = "dwolla_sandbox"
            snapshot["balance_cents"] = 0
            snapshot["payments"] = []
            snapshot["plaid_configured"] = true
            snapshot["banking"] = ["status": "not_started", "blocked": "", "debt_cents": 0, "bank_status": "not_started"] as [String: Any]
            snapshot["fundings"] = []
            snapshot["reserved_payment_ids"] = []
            if ProcessInfo.processInfo.arguments.contains("--preview-bank-linked") {
                let friend = nearby[0]
                let created: CreatedPayment = try core.call(["op": "create_payment", "ledger_id": saved.snapshot!.ledgerId, "recipient": Wire.object(friend.profile), "amount": "25.00", "memo": "Coffee", "balance_cents": 12500, "pending": []])
                saved.journal = [LocalPayment(payment: created.payment, packet: created.packet, friendName: "Sam", state: "pending")]
                snapshot["balance_cents"] = 10000
                snapshot["bank"] = ["label": "Sandbox checking", "mask": "1234", "mode": "dwolla_sandbox"]
                snapshot["banking"] = ["status": "verified", "blocked": "", "debt_cents": 0, "bank_status": "ready"] as [String: Any]
                snapshot["reserved_payment_ids"] = [created.payment.body.id]
                snapshot["payments"] = [["id": created.payment.body.id, "sender": publicKey, "recipient": friend.id, "amount_cents": 2500, "memo": "Coffee", "settled_at": created.payment.body.createdAt, "sender_name": "Dylan", "recipient_name": "Sam", "status": "submitted"] as [String: Any]]
            }
            if ProcessInfo.processInfo.arguments.contains("--preview-link-interrupted") || ProcessInfo.processInfo.arguments.contains("--preview-link-expired") {
                let interrupted = ProcessInfo.processInfo.arguments.contains("--preview-link-interrupted")
                snapshot["banking"] = ["status": "verified", "blocked": "", "debt_cents": 0, "bank_status": interrupted ? "not_started" : "relink_required"] as [String: Any]
                saved.bankLinkInProgress = interrupted; bankLinkInterrupted = interrupted
            }
            saved.snapshot = try Wire.decoder.decode(WalletSnapshot.self, from: JSONSerialization.data(withJSONObject: snapshot))
        }
        online = true; bluetoothOn = true; bluetoothStatus = "3 people nearby"
    }
}

import Combine

private func previewSnapshot(publicKey: String) -> String {
    """
    {"ledger_id":"00000000-0000-0000-0000-000000000001","public_key":"\(publicKey)","name":"Dylan","balance_cents":12540,"currency":"USD","mode":"sandbox","plaid_configured":false,"bank":null,"payments":[{"id":"preview-1","sender":"sam","recipient":"\(publicKey)","amount_cents":1800,"memo":"Lunch, on me","settled_at":1792623300,"sender_name":"Sam","recipient_name":"Dylan","status":"settled"},{"id":"preview-2","sender":"\(publicKey)","recipient":"alex","amount_cents":650,"memo":"Coffee run","settled_at":1792619700,"sender_name":"Dylan","recipient_name":"Alex","status":"settled"}],"withdrawals":[]}
    """
}
