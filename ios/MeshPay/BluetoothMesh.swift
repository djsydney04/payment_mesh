import CoreBluetooth
import Foundation

/// Apple adapter only. Rust owns framing, validation, decryption, and forwarding rules.
@MainActor
final class BluetoothMesh: NSObject, ObservableObject {
    static let serviceID = CBUUID(string: "8F650001-4D45-5348-5041-590000000001")
    private static let profileID = CBUUID(string: "8F650002-4D45-5348-5041-590000000001")
    private static let packetID = CBUUID(string: "8F650003-4D45-5348-5041-590000000001")
    @Published var people: [NearbyPerson] = []
    @Published var status = "Starting Bluetooth"
    @Published var poweredOn = false
    var onPayment: ((Payment, MeshPacket) -> Void)?
    var onConnected: (() -> Void)?
    var onFailure: ((String) -> Void)?
    private let core: RustCore
    private var profile: PeerProfile
    private var central: CBCentralManager!
    private var peripheralManager: CBPeripheralManager!
    private var packetCharacteristic: CBMutableCharacteristic?
    private var devices: [UUID: CBPeripheral] = [:]
    private var packetCharacteristics: [UUID: CBCharacteristic] = [:]
    private var deviceKeys: [UUID: String] = [:]
    private var writes: [UUID: [Data]] = [:]
    private var writing: Set<UUID> = []
    private var subscribers: [UUID: CBCentral] = [:]
    private var notifications: [(Data, CBCentral)] = []
    private var relays: [MeshPacket] = []
    private var running = false
    private var pruneTimer: Timer?

    init(core: RustCore, profile: PeerProfile) {
        self.core = core; self.profile = profile
        super.init()
        central = CBCentralManager(delegate: self, queue: .main)
        peripheralManager = CBPeripheralManager(delegate: self, queue: .main)
        pruneTimer = Timer.scheduledTimer(withTimeInterval: 20, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.prune() }
        }
    }
    func start() { running = true; configureRadio() }
    func updateProfile(_ profile: PeerProfile) { self.profile = profile }
    func broadcast(_ packet: MeshPacket) {
        guard poweredOn else { return }
        for (id, device) in devices where device.state == .connected && packetCharacteristics[id] != nil {
            let mtu = min(512, device.maximumWriteValueLength(for: .withResponse))
            guard mtu >= 40, let frames = frames(packet, mtu: mtu), (writes[id]?.count ?? 0) + frames.count <= 4096 else { continue }
            writes[id, default: []].append(contentsOf: frames)
            flushWrite(id)
        }
        for subscriber in subscribers.values {
            let mtu = min(512, subscriber.maximumUpdateValueLength)
            guard mtu >= 40, let frames = frames(packet, mtu: mtu), notifications.count + frames.count <= 4096 else { continue }
            notifications.append(contentsOf: frames.map { ($0, subscriber) })
        }
        flushNotifications()
    }
    private func frames(_ packet: MeshPacket, mtu: Int) -> [Data]? {
        guard let object = try? Wire.object(packet), let result = try? core.call(["op": "fragment", "packet": object, "mtu": mtu], as: FragmentResult.self) else { return nil }
        return result.frames.compactMap { Data(base64Encoded: $0) }
    }
    private func configureRadio() {
        poweredOn = central.state == .poweredOn && peripheralManager.state == .poweredOn
        if central.state == .unauthorized || peripheralManager.state == .unauthorized { status = "Allow Bluetooth in Settings"; return }
        if central.state == .unsupported || peripheralManager.state == .unsupported { status = "Bluetooth needs a real iPhone"; return }
        guard running else { return }
        guard poweredOn else { status = "Turn on Bluetooth to find friends"; return }
        status = "Looking for people nearby"
        central.scanForPeripherals(withServices: [Self.serviceID], options: [CBCentralManagerScanOptionAllowDuplicatesKey: true])
        if packetCharacteristic == nil {
            let service = CBMutableService(type: Self.serviceID, primary: true)
            let profile = CBMutableCharacteristic(type: Self.profileID, properties: [.read], value: nil, permissions: [.readable])
            let packets = CBMutableCharacteristic(type: Self.packetID, properties: [.write, .notify], value: nil, permissions: [.writeable])
            packetCharacteristic = packets
            service.characteristics = [profile, packets]
            peripheralManager.add(service)
        }
    }
    private func flushWrite(_ id: UUID) {
        guard !writing.contains(id), let device = devices[id], let characteristic = packetCharacteristics[id], let data = writes[id]?.first else { return }
        writing.insert(id)
        device.writeValue(data, for: characteristic, type: .withResponse)
    }
    private func flushNotifications() {
        guard let characteristic = packetCharacteristic else { return }
        while let (frame, subscriber) = notifications.first {
            guard peripheralManager.updateValue(frame, for: characteristic, onSubscribedCentrals: [subscriber]) else { return }
            notifications.removeFirst()
        }
    }
    private func receive(_ data: Data, source: String) {
        do {
            let result: ReceivedFrame = try core.call(["op": "receive_frame", "source": source, "frame": data.base64EncodedString()])
            if let payment = result.payment, let packet = result.packet { onPayment?(payment, packet) }
            if let forward = result.forward {
                prune()
                if relays.count < 128 { relays.append(forward); broadcast(forward) }
            }
        } catch {
            // Malformed or unauthenticated radio traffic is discarded before reaching the wallet.
        }
    }
    private func prune() {
        people.removeAll { Date().timeIntervalSince($0.lastSeen) > 65 }
        relays.removeAll { $0.envelope.body.expiresAt <= Int64(Date().timeIntervalSince1970) }
        if poweredOn { status = people.isEmpty ? "Looking for people nearby" : "\(people.count) \(people.count == 1 ? "person" : "people") nearby" }
    }
}

extension BluetoothMesh: @preconcurrency CBCentralManagerDelegate, @preconcurrency CBPeripheralDelegate {
    func centralManagerDidUpdateState(_ central: CBCentralManager) { configureRadio() }
    func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral, advertisementData: [String: Any], rssi RSSI: NSNumber) {
        if let key = deviceKeys[peripheral.identifier], let index = people.firstIndex(where: { $0.id == key }) {
            people[index].lastSeen = Date(); people[index].signal = RSSI.intValue
        }
        guard devices[peripheral.identifier] == nil, devices.count < 8 else { return }
        devices[peripheral.identifier] = peripheral
        peripheral.delegate = self
        central.connect(peripheral)
    }
    func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) { peripheral.discoverServices([Self.serviceID]) }
    func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) { disconnect(peripheral) }
    func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) { disconnect(peripheral) }
    private func disconnect(_ peripheral: CBPeripheral) {
        let id = peripheral.identifier
        devices.removeValue(forKey: id); packetCharacteristics.removeValue(forKey: id)
        writes.removeValue(forKey: id); writing.remove(id)
        if let key = deviceKeys.removeValue(forKey: id) { people.removeAll { $0.id == key } }
        prune()
    }
    func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard error == nil else { return }
        for service in peripheral.services ?? [] { peripheral.discoverCharacteristics([Self.profileID, Self.packetID], for: service) }
    }
    func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        guard error == nil else { return }
        for characteristic in service.characteristics ?? [] {
            if characteristic.uuid == Self.profileID { peripheral.readValue(for: characteristic) }
            if characteristic.uuid == Self.packetID {
                packetCharacteristics[peripheral.identifier] = characteristic
                peripheral.setNotifyValue(true, for: characteristic)
            }
        }
    }
    func peripheral(_ peripheral: CBPeripheral, didUpdateNotificationStateFor characteristic: CBCharacteristic, error: Error?) {
        guard error == nil, characteristic.isNotifying else { return }
        onConnected?()
        for packet in relays { broadcast(packet) }
    }
    func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        guard error == nil, let data = characteristic.value, data.count <= 16_384 else { return }
        if characteristic.uuid == Self.profileID {
            do {
                let object = try JSONSerialization.jsonObject(with: data)
                struct Verified: Decodable { let profile: PeerProfile; let fingerprint: String }
                let result: Verified = try core.call(["op": "verify_profile", "profile": object])
                guard result.profile.body.publicKey != profile.body.publicKey else { return }
                let person = NearbyPerson(profile: result.profile, fingerprint: result.fingerprint, lastSeen: Date(), signal: -60)
                deviceKeys[peripheral.identifier] = person.id
                if let i = people.firstIndex(where: { $0.id == person.id }) { people[i] = person } else { people.append(person) }
                prune()
            } catch { central.cancelPeripheralConnection(peripheral) }
        } else if characteristic.uuid == Self.packetID { receive(data, source: peripheral.identifier.uuidString) }
    }
    func peripheral(_ peripheral: CBPeripheral, didWriteValueFor characteristic: CBCharacteristic, error: Error?) {
        let id = peripheral.identifier
        writing.remove(id)
        if error != nil { writes[id] = []; return }
        if writes[id]?.isEmpty == false { writes[id]?.removeFirst() }
        flushWrite(id)
    }
}

extension BluetoothMesh: @preconcurrency CBPeripheralManagerDelegate {
    func peripheralManagerDidUpdateState(_ peripheral: CBPeripheralManager) {
        if peripheral.state != .poweredOn {
            packetCharacteristic = nil; subscribers = [:]; notifications = []
        }
        configureRadio()
    }
    func peripheralManager(_ peripheral: CBPeripheralManager, didAdd service: CBService, error: Error?) {
        guard error == nil else { status = "Bluetooth couldn't start. Try reopening Mesh."; return }
        peripheral.startAdvertising([CBAdvertisementDataServiceUUIDsKey: [Self.serviceID]])
    }
    func peripheralManager(_ peripheral: CBPeripheralManager, didReceiveRead request: CBATTRequest) {
        guard request.characteristic.uuid == Self.profileID, let data = try? Wire.encoder.encode(profile) else { peripheral.respond(to: request, withResult: .readNotPermitted); return }
        guard request.offset <= data.count else { peripheral.respond(to: request, withResult: .invalidOffset); return }
        request.value = data.subdata(in: request.offset..<data.count)
        peripheral.respond(to: request, withResult: .success)
    }
    func peripheralManager(_ peripheral: CBPeripheralManager, didReceiveWrite requests: [CBATTRequest]) {
        guard let first = requests.first else { return }
        for request in requests {
            guard request.characteristic.uuid == Self.packetID, request.offset == 0, let data = request.value, data.count <= 512 else { peripheral.respond(to: first, withResult: .invalidAttributeValueLength); return }
        }
        for request in requests { if let data = request.value { receive(data, source: request.central.identifier.uuidString) } }
        peripheral.respond(to: first, withResult: .success)
    }
    func peripheralManager(_ peripheral: CBPeripheralManager, central: CBCentral, didSubscribeTo characteristic: CBCharacteristic) {
        subscribers[central.identifier] = central
        onConnected?()
        for packet in relays { broadcast(packet) }
    }
    func peripheralManager(_ peripheral: CBPeripheralManager, central: CBCentral, didUnsubscribeFrom characteristic: CBCharacteristic) {
        subscribers.removeValue(forKey: central.identifier)
        notifications.removeAll { $0.1.identifier == central.identifier }
    }
    func peripheralManagerIsReady(toUpdateSubscribers peripheral: CBPeripheralManager) { flushNotifications() }
}
