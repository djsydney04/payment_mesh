import XCTest

final class MeshPayUITests: XCTestCase {
    override func setUpWithError() throws { continueAfterFailure = false }
    @MainActor
    func testNearbyPaymentAndBankScreens() throws {
        let app = XCUIApplication()
        app.launchArguments = ["--preview"]
        app.launch()
        XCTAssertTrue(app.buttons["home.pay"].waitForExistence(timeout: 15))
        XCTAssertTrue(app.staticTexts["$125.40"].exists)
        XCTAssertTrue(app.buttons["nearby.Sam"].exists)
        attach("Nearby", app: app)
        app.buttons["nearby.Sam"].tap()
        XCTAssertTrue(app.staticTexts["For Sam"].waitForExistence(timeout: 5))
        for key in ["1", "2", ".", "5", "0"] { app.buttons["keypad.\(key)"].tap() }
        XCTAssertTrue(app.staticTexts["12.50"].exists)
        attach("Payment", app: app)
        app.buttons["Back"].tap()
        app.buttons["home.cashout"].tap()
        XCTAssertTrue(app.buttons["bank.plaid"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["bank.plaid"].isEnabled)
        XCTAssertTrue(app.buttons["bank.demo"].exists)
        attach("Bank", app: app)
        app.buttons["Close"].tap()
        app.buttons["Activity"].tap()
        XCTAssertTrue(app.staticTexts["Your activity"].waitForExistence(timeout: 5))
    }
    @MainActor
    func testProviderWalletSetupScreen() throws {
        let app = XCUIApplication(); app.launchArguments = ["--preview", "--preview-bank"]; app.launch()
        XCTAssertTrue(app.buttons["home.cashout"].waitForExistence(timeout: 15))
        XCTAssertFalse(app.buttons["Add $125.40 in test money"].exists)
        app.buttons["home.cashout"].tap()
        XCTAssertTrue(app.buttons["bank.verify"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["bank.verify"].isEnabled)
        XCTAssertFalse(app.buttons["bank.demo"].exists)
        attach("Provider setup", app: app)
    }
    @MainActor
    func testProviderReservationIsNotSubtractedTwiceAndBankActionsAreClear() throws {
        let app = XCUIApplication(); app.launchArguments = ["--preview", "--preview-bank", "--preview-bank-linked"]; app.launch()
        XCTAssertTrue(app.buttons["home.cashout"].waitForExistence(timeout: 15))
        XCTAssertTrue(app.staticTexts["$100.00"].exists)
        XCTAssertTrue(app.staticTexts["Pending"].exists)
        app.buttons["home.cashout"].tap()
        XCTAssertTrue(app.staticTexts["Sandbox checking"].waitForExistence(timeout: 5))
        app.buttons["Add money"].tap()
        XCTAssertTrue(app.buttons["Authorize test deposit"].exists)
        XCTAssertTrue(app.staticTexts["One-time debit from your linked test bank"].exists)
        attach("Provider bank", app: app)
    }
    @MainActor
    func testInterruptedBankSignInHasAClearRestartPath() throws {
        let app = XCUIApplication(); app.launchArguments = ["--preview", "--preview-bank", "--preview-link-interrupted"]; app.launch()
        XCTAssertTrue(app.buttons["bank.plaid"].waitForExistence(timeout: 15))
        XCTAssertTrue(app.otherElements["bank.reconnect"].exists || app.staticTexts["Bank sign-in was interrupted. Start again below to reconnect securely."].exists)
        XCTAssertFalse(app.buttons["bank.demo"].exists)
        attach("Interrupted bank sign-in", app: app)
        app.buttons["Close"].tap(); app.buttons["home.cashout"].tap()
        XCTAssertTrue(app.buttons["bank.plaid"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.staticTexts["Bank sign-in was interrupted. Start again below to reconnect securely."].exists)
    }
    @MainActor
    func testExpiredBankSignInCanStartAgainWithoutFreezingTheWallet() throws {
        let app = XCUIApplication(); app.launchArguments = ["--preview", "--preview-bank", "--preview-link-expired"]; app.launch()
        XCTAssertTrue(app.buttons["home.cashout"].waitForExistence(timeout: 15)); app.buttons["home.cashout"].tap()
        XCTAssertTrue(app.buttons["bank.plaid"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Your bank sign-in expired. Connect with Plaid again below."].exists)
        XCTAssertFalse(app.staticTexts["Your wallet needs a bank review. Transfers stay visible in Activity."].exists)
        attach("Expired bank sign-in", app: app)
    }
    private func attach(_ name: String, app: XCUIApplication) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name; attachment.lifetime = .keepAlways; add(attachment)
    }

    @MainActor
    func testWalletPersistsAcrossRelaunch() throws {
        let app = XCUIApplication()
        app.launch()
        let name = app.textFields["onboarding.name"]
        if name.waitForExistence(timeout: 5) {
            name.tap(); name.typeText("Taylor")
            app.buttons["Let's get nearby"].tap()
        }
        XCTAssertTrue(app.buttons["home.pay"].waitForExistence(timeout: 15))
        app.terminate(); app.launch()
        XCTAssertTrue(app.buttons["home.pay"].waitForExistence(timeout: 15))
        XCTAssertFalse(app.staticTexts["Wallet needs attention"].exists)
    }

    @MainActor
    func testNativeSandboxFundingAndBankLink() async throws {
        // Run against the actual local Rust server when it is available.
        let health = URL(string: "http://localhost:8787/health")!
        do {
            let (data, _) = try await URLSession.shared.data(from: health)
            let value = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            guard value?["mode"] as? String == "sandbox" else { throw XCTSkip("Local Mesh sandbox is not running") }
        } catch { throw XCTSkip("Start cargo run -p mesh-server to run the native API integration test") }
        let app = XCUIApplication()
        app.launch()
        let name = app.textFields["onboarding.name"]
        if name.waitForExistence(timeout: 3) {
            name.tap(); name.typeText("Taylor")
            app.buttons["Let's get nearby"].tap()
        }
        XCTAssertTrue(app.buttons["home.pay"].waitForExistence(timeout: 10))
        let fund = app.buttons["Add $125.40 in test money"]
        if fund.waitForExistence(timeout: 5) { fund.tap() }
        XCTAssertTrue(app.staticTexts["$125.40"].waitForExistence(timeout: 10))
        app.buttons["home.cashout"].tap()
        let demo = app.buttons["bank.demo"]
        if demo.waitForExistence(timeout: 3) { demo.tap() }
        XCTAssertTrue(app.staticTexts["Demo checking"].waitForExistence(timeout: 10))
        app.buttons["Close"].tap()
        app.terminate(); app.launch()
        XCTAssertTrue(app.staticTexts["$125.40"].waitForExistence(timeout: 10))
    }
}
