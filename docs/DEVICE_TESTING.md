# Physical device verification

Automated tests exercise the Rust wire protocol, HTTP/ledger behavior and simulator screen flow. They do not prove successful BLE delivery between two physical iPhones, background reliability, Face ID confirmation or the Plaid OAuth app-switch return path. Run this device pass before inviting external testers.

| Scenario | Expected behavior |
|---|---|
| First launch without Bluetooth permission | Clear permission status; no fake nearby people |
| Two foreground phones, Bluetooth on | Both discover the other signed profile within radio range |
| Same display name on two phones | Different identity codes; comparing codes disambiguates |
| Online $12.50 payment | One debit and one credit; both see the same transfer ID |
| Wi-Fi/cellular off, Bluetooth on | Receiver records a pending payment; received money is not spendable |
| Only receiver regains internet | Receiver submits; sender balance is updated on next sync |
| Sender app terminated after confirmation | Persisted outgoing instruction survives restart |
| Duplicate radio delivery / HTTP retry | Exactly one financial settlement |
| Two offline sends exceeding real balance | Ledger accepts at most the funded amount; declined instruction remains declined |
| Friend leaves range mid-fragment | No partial payment is credited; retry uses persisted instruction |
| Three phones A→B→C, A/C out of range | B forwards confidential packets within four honest hops; C decrypts |
| Bluetooth off/on | Managers recover, peers disappear/reappear, pending work remains saved |
| Both apps backgrounded / locked | Delivery may be delayed; app never implies guaranteed settlement |
| 24-hour-old instruction | New settlement rejected; existing settlement receipt remains readable |
| Cash-out HTTP response interrupted | App retries original withdrawal ID; only one reserve debit |
| Server restart with a queued payout | Worker resumes and completes once; ledger audit passes |
| Corrupt local journal / inaccessible Keychain | Error shown; existing identity/journal preserved |
| Plaid Sandbox bank link | Correct account displayed; no access token in client storage or logs |
| Plaid bank OAuth return | Domain association, app-switch return and interruption behavior verified |
| Large text / small iPhone | Scrollable screens, accessible buttons and readable pending/error states |

Both devices must share one ledger database. Switching to another backend creates a distinct ledger UUID; old instructions must be rejected there. Keep the server on a trusted local network for this sandbox pass.
