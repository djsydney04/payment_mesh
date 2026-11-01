# Banking partner recommendation

Reviewed October 21, 2026. Assumption: a US launch, USD, adults paying friends, a stored wallet balance and bank cash outs. This is a technical shortlist, not confirmation that a provider has accepted Mesh.

**Start with Dwolla Balance + Plaid Auth.** It is the closest published fit to the current product: Dwolla describes stored balances, payments between users and withdrawals to linked bank accounts. Its developer documentation supports personal verified customers holding balances. Plaid documents a direct processor-token integration. This recommendation is an inference from those capabilities; production approval and the commercial quote remain open.

| Candidate | Why it fits | What to establish first |
|---|---|---|
| **Dwolla Balance — first choice** | Stored wallet balances, transfers between users, linked-bank funding/cash outs; documented Plaid integration | Written approval for consumer P2P and delayed submission of offline signed instructions, customer verification flow, minimum monthly spend, reserves and return liability |
| **Increase — alternative for a broader banking product** | Published wallet/account offering, ACH and real-time rails, individual bank accounts or program-dependent FBO structure | Sponsor-bank program approval, required operating controls, platform quote and whether individual accounts are preferable to a pooled wallet |

Sources: [Dwolla digital wallets](https://www.dwolla.com/features/digital-wallet), [Dwolla customer types](https://developers.dwolla.com/docs/customer-types), [Plaid + Dwolla](https://plaid.com/docs/auth/partnerships/dwolla/), [Increase wallets](https://increase.com/solutions/wallets), [Increase program setup](https://www.increase.com/documentation/bill-payment-programs).

## Cost decision

Neither provider's public site gives a binding all-in quote for this consumer wallet program. Dwolla currently describes custom volume/rail/integration pricing. Increase says fintech platforms have custom pricing and monthly charges depend on use case. Do not infer that either is a low-cost, self-serve launch just because Sandbox access is easy. Ask both for the same monthly minimum, implementation cost, per-user verification fee, ACH debit/credit fee, wallet-transfer fee, faster-payout fee, return fee and reserve requirements. Compare the total at your actual expected number of active users and bank transfers.

Sources: [Dwolla pricing](https://www.dwolla.com/pricing), [Increase fees](https://increase.com/fees). Bank fees and commercial terms must be confirmed directly.

## Recommended funds flow

1. Complete provider-approved customer verification online before a wallet can hold real value. A nearby nickname can remain the everyday display name. Dwolla's documented stored-balance model requires verified customers; its customer types are for US persons aged 18 or older.
2. Link the external bank with Plaid Auth and exchange a processor token on the Rust server. Do not transmit account/routing numbers over Bluetooth.
3. Fund the wallet through the approved provider. Only make funds available under the provider's confirmed availability and ACH-return policy.
4. Bluetooth delivers an encrypted signed instruction. The UI keeps it pending until online validation and the provider-confirmed wallet transfer. There is no claim of final offline bank settlement.
5. Cash out from the provider-backed wallet to the linked bank. Track submission, settlement, failures and returns with verified webhooks plus periodic reconciliation.

The existing Rust ledger is a sandbox ledger. Under a Dwolla-backed model it should become a reconciled mirror of provider balances and transfer states, not independently manufacture spendable dollars when a local database transaction succeeds. A new live flow needs an outbox and provider acknowledgements before showing final completion.

## Questions for Dwolla before committing

- Do you approve a US consumer wallet for friends paying friends, including instructions exchanged by Bluetooth and submitted later? Only online-validated payments would become final.
- Can our verified personal customers hold balances, transfer between one another, fund from their own banks, and cash out using Plaid Auth processor tokens?
- Which customer disclosures, ACH authorizations, identity checks, fraud controls and stored-value requirements apply to this exact model?
- What are the minimum monthly commitment, onboarding/implementation costs, per-user fees, transfer fees, reserves, ACH-return liability and termination/export arrangements?
- What provider event constitutes available funds, final wallet transfer and completed cash out? How do delayed returns affect balances and reserves?

## Sandbox implementation

The Dwolla Sandbox transport, customer enrollment, bank setup, durable transfer outbox, event-driven wallet accounting, funding, cash outs, return/debt handling and conservative review freezes are implemented. Select `MESH_RAIL=dwolla_sandbox` with a new database and your Sandbox credentials; see [the provider guide](BANK_PROVIDER.md).

Local scripted-provider and accounting tests pass. External account eligibility, actual bank Link/provider transfers, physical Bluetooth behavior and production commercial approval remain unverified. Returned peer/cash-out operations require reviewed operational recovery. Production is disabled.
