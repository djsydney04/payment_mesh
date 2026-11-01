import Foundation

struct ProfileBody: Codable, Equatable {
    let version: Int
    let publicKey: String
    let encryptionKey: String
    let name: String
    let createdAt: Int64
}
struct PeerProfile: Codable, Equatable {
    let body: ProfileBody
    let signature: String
}
struct ProfileResult: Decodable {
    let profile: PeerProfile
    let fingerprint: String
    let publicKey: String
}
struct PaymentBody: Codable, Equatable {
    let version: Int
    let ledgerId: String
    let id: String
    let sender: String
    let recipient: String
    let amountCents: Int64
    let currency: String
    let memo: String
    let createdAt: Int64
    let expiresAt: Int64
}
struct Payment: Codable, Equatable {
    let body: PaymentBody
    let signature: String
}
struct EnvelopeBody: Codable {
    let version: Int
    let id: String
    let sender: String
    let recipient: String
    let expiresAt: Int64
    let ephemeralKey: String
    let nonce: String
    let ciphertext: String
}
struct Envelope: Codable { let body: EnvelopeBody; let signature: String }
struct MeshPacket: Codable { let hopsRemaining: Int; let envelope: Envelope }
struct CreatedPayment: Decodable { let payment: Payment; let packet: MeshPacket }
struct ReceivedFrame: Decodable { let packet: MeshPacket?; let payment: Payment?; let forward: MeshPacket? }
struct FragmentResult: Decodable { let frames: [String] }
struct SpendableResult: Decodable { let amountCents: Int64 }
struct EmptyResponse: Decodable {}
struct Receipt: Decodable { let id: String; let status: String }
struct LinkToken: Decodable { let linkToken: String }
struct BankLinkReceipt: Decodable { let id: String?; let status: String? }
struct BankLinkAttempt: Codable { let id: String; let publicToken: String; let accountId: String }

struct Bank: Codable {
    let label: String
    let mask: String
    let mode: String
}
struct SettledPayment: Codable, Identifiable {
    let id: String
    let sender: String
    let recipient: String
    let amountCents: Int64
    let memo: String
    let settledAt: Int64
    let senderName: String
    let recipientName: String
    let status: String
}
struct Withdrawal: Codable, Identifiable {
    let id: String
    let amountCents: Int64
    let status: String
    let bankLabel: String
    let bankMask: String
    let createdAt: Int64
}
struct BankingStatus: Codable {
    let status: String
    let blocked: String
    let debtCents: Int64
    let bankStatus: String?
}
struct WalletSnapshot: Codable {
    let ledgerId: String
    let publicKey: String
    let name: String
    let balanceCents: Int64
    let currency: String
    let mode: String
    let plaidConfigured: Bool
    let bank: Bank?
    let payments: [SettledPayment]
    let withdrawals: [Withdrawal]
    let bankRail: String?
    let banking: BankingStatus?
    let fundings: [Withdrawal]?
    let reservedPaymentIds: [String]?
}
struct LocalPayment: Codable, Identifiable {
    var id: String { payment.body.id }
    let payment: Payment
    let packet: MeshPacket
    let friendName: String
    var state: String // pending, settled, failed
    var failure: String?
}
struct CashoutAttempt: Codable { let id: String; let amountCents: Int64 }
struct SavedWallet: Codable {
    var name = ""
    var onboarded = false
    var serverUrl = "http://localhost:8787"
    var snapshot: WalletSnapshot?
    var journal: [LocalPayment] = []
    var cashout: CashoutAttempt?
    var funding: CashoutAttempt?
    var enrollmentId: String?
    var bankLinkAttempt: BankLinkAttempt?
    var bankLinkInProgress: Bool?
}
struct NearbyPerson: Identifiable {
    var id: String { profile.body.publicKey }
    let profile: PeerProfile
    let fingerprint: String
    var lastSeen: Date
    var signal: Int
}
struct ActivityItem: Identifiable {
    let id: String
    let name: String
    let detail: String
    let amount: Int64
    let incoming: Bool
    let status: String
    let at: Int64
    let isBank: Bool
}

enum MeshError: LocalizedError {
    case message(String)
    case api(String, String)
    var errorDescription: String? {
        switch self { case .message(let text): return text; case .api(_, let text): return text }
    }
    var code: String? { if case .api(let code, _) = self { return code }; return nil }
}

enum Wire {
    static let encoder: JSONEncoder = { let e = JSONEncoder(); e.keyEncodingStrategy = .convertToSnakeCase; return e }()
    static let decoder: JSONDecoder = { let d = JSONDecoder(); d.keyDecodingStrategy = .convertFromSnakeCase; return d }()
    static func object<T: Encodable>(_ value: T) throws -> Any { try JSONSerialization.jsonObject(with: encoder.encode(value)) }
}

func money(_ cents: Int64) -> String {
    let value = Decimal(cents) / 100
    return value.formatted(.currency(code: "USD"))
}
