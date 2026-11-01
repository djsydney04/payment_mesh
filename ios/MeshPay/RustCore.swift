import Foundation
import Security

/// The handle is confined to the main actor, including all CoreBluetooth callbacks.
@MainActor
final class RustCore {
    private let handle: OpaquePointer
    init() throws {
        let seed = try Self.loadOrCreateSeed()
        guard let handle = seed.withCString({ mesh_engine_new($0) }) else { throw MeshError.message("Your wallet key could not be opened. Your saved wallet has been preserved.") }
        self.handle = handle
    }
    deinit { mesh_engine_free(handle) }
    func call<T: Decodable>(_ command: [String: Any], as type: T.Type = T.self) throws -> T {
        let data = try JSONSerialization.data(withJSONObject: command, options: [.sortedKeys])
        guard let text = String(data: data, encoding: .utf8), let result = text.withCString({ mesh_engine_call(handle, $0) }) else { throw MeshError.message("The payment engine is unavailable.") }
        defer { mesh_string_free(result) }
        let response = Data(String(cString: result).utf8)
        guard let object = try JSONSerialization.jsonObject(with: response) as? [String: Any], object["ok"] as? Bool == true, let value = object["data"] else {
            let object = try? JSONSerialization.jsonObject(with: response) as? [String: Any]
            throw MeshError.message(object?["error"] as? String ?? "The payment could not be created.")
        }
        return try Wire.decoder.decode(T.self, from: JSONSerialization.data(withJSONObject: value))
    }
    func request(action: String, payload: Any) throws -> Data {
        let data = try JSONSerialization.data(withJSONObject: ["op": "request", "action": action, "payload": payload])
        let input = String(decoding: data, as: UTF8.self)
        guard let result = input.withCString({ mesh_engine_call(handle, $0) }) else { throw MeshError.message("Could not sign request.") }
        defer { mesh_string_free(result) }
        let object = try JSONSerialization.jsonObject(with: Data(String(cString: result).utf8)) as? [String: Any]
        guard object?["ok"] as? Bool == true, let body = object?["data"] else { throw MeshError.message("Could not sign request.") }
        return try JSONSerialization.data(withJSONObject: body)
    }
    private static func loadOrCreateSeed() throws -> String {
        let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "app.mesh.wallet", kSecAttrAccount as String: "identity-v1", kSecReturnData as String: true]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecSuccess, let data = result as? Data, let seed = String(data: data, encoding: .utf8) { return seed }
        guard status == errSecItemNotFound else { throw MeshError.message("Unlock your iPhone to open your wallet.") }
        guard let pointer = mesh_seed_new() else { throw MeshError.message("Could not create a wallet key.") }
        defer { mesh_string_free(pointer) }
        let seed = String(cString: pointer)
        var add = query
        add.removeValue(forKey: kSecReturnData as String)
        add[kSecValueData as String] = Data(seed.utf8)
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        guard SecItemAdd(add as CFDictionary, nil) == errSecSuccess else { throw MeshError.message("Your wallet key could not be saved securely.") }
        return seed
    }
}

@MainActor
final class WalletDisk {
    private let url: URL
    init() throws {
        let directory = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true).appendingPathComponent("Mesh", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        url = directory.appendingPathComponent("wallet-v1.json")
    }
    func load() throws -> SavedWallet {
        guard FileManager.default.fileExists(atPath: url.path) else { return SavedWallet() }
        do { return try Wire.decoder.decode(SavedWallet.self, from: Data(contentsOf: url)) }
        catch { throw MeshError.message("Your saved wallet could not be read. It has been preserved; restore it before continuing.") }
    }
    func save(_ state: SavedWallet) throws {
        let data = try Wire.encoder.encode(state)
        try data.write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        var file = url
        try file.setResourceValues(values)
    }
}
