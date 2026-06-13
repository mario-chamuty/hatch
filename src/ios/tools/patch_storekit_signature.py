#!/usr/bin/env python3
"""Augment the iPhoneOS16.5 StoreKit.swiftinterface with the genuine iOS 17.4 /
iOS 18.0 StoreKit API surface that in_app_purchase_storekit references but that
the 16.5 SDK predates. Each is declared EXACTLY as Apple declares it (same
module / type path / labels), so the compiler mangles calls to StoreKit's own
real symbols. Every usage is `@available(iOS 17.4|18.0)`-gated, so it only
executes on devices whose StoreKit dylib actually exports the symbol; on older
iOS the code path is never taken. Same reconstruction approach as the Dispatch
apinotes fix - give the toolchain Apple's real API description, never a
competing local definition (a shim module would resolve calls to the shim and
crash real purchases).

Additions:
  iOS 17.4: Product.SubscriptionOffer.Signature (struct + init)
            Product.PurchaseOption.promotionalOffer(offerID:signature:)
  iOS 18.0: Product.SubscriptionOffer.OfferType.winBack
            Product.SubscriptionInfo.winBackOffers
            Product.SubscriptionInfo.RenewalInfo.eligibleWinBackOfferIDs
            Product.PurchaseOption.winBackOffer(_:)

Idempotent. Patches every <arch>-apple-ios.swiftinterface under the SDK's
StoreKit.framework.
"""
import glob, os, sys

SDK = sys.argv[1] if len(sys.argv) > 1 else r"C:\Users\mario\iospoc-win\iossdk\iPhoneOS16.5.sdk"
ifaces = glob.glob(os.path.join(
    SDK, "System", "Library", "Frameworks", "StoreKit.framework",
    "Modules", "StoreKit.swiftmodule", "*-apple-ios.swiftinterface"))

A174 = "@available(iOS 17.4, macOS 14.4, tvOS 17.4, watchOS 10.4, visionOS 1.1, *)"
A18 = "@available(iOS 18.0, macOS 15.0, tvOS 18.0, watchOS 11.0, visionOS 2.0, *)"

# (uniqueness-probe, anchor-line-inserted-after, text-to-insert)
EDITS = [
    # --- iOS 17.4 ---
    ("public struct Signature {",
     "    public let paymentMode: StoreKit.Product.SubscriptionOffer.PaymentMode\n",
     "    " + A174 + "\n"
     "    public struct Signature {\n"
     "      public init(keyID: Swift.String, nonce: Foundation.UUID, timestamp: Swift.Int, signature: Foundation.Data)\n"
     "    }\n"),
    ("offerID: Swift.String, signature: StoreKit.Product.SubscriptionOffer.Signature",
     "    public static func promotionalOffer(offerID: Swift.String, keyID: Swift.String, nonce: Foundation.UUID, signature: Foundation.Data, timestamp: Swift.Int) -> StoreKit.Product.PurchaseOption\n",
     "    " + A174 + "\n"
     "    public static func promotionalOffer(offerID: Swift.String, signature: StoreKit.Product.SubscriptionOffer.Signature) -> StoreKit.Product.PurchaseOption\n"),
    ("public static func winBackOffer(",
     "    public static func quantity(_ quantity: Swift.Int) -> StoreKit.Product.PurchaseOption\n",
     "    " + A18 + "\n"
     "    public static func winBackOffer(_ offer: StoreKit.Product.SubscriptionOffer) -> StoreKit.Product.PurchaseOption\n"),
    # --- iOS 18.0 ---
    ("public static let winBack: StoreKit.Product.SubscriptionOffer.OfferType",
     "      public static let promotional: StoreKit.Product.SubscriptionOffer.OfferType\n",
     "      " + A18 + "\n"
     "      public static let winBack: StoreKit.Product.SubscriptionOffer.OfferType\n"),
    ("public var winBackOffers:",
     "    public let promotionalOffers: [StoreKit.Product.SubscriptionOffer]\n",
     "    " + A18 + "\n"
     "    public var winBackOffers: [StoreKit.Product.SubscriptionOffer] {\n      get\n    }\n"),
    ("public var eligibleWinBackOfferIDs:",
     "  public struct RenewalInfo {\n",
     "    " + A18 + "\n"
     "    public var eligibleWinBackOfferIDs: [Swift.String] {\n      get\n    }\n"),
]

def patch(path):
    s = open(path, encoding="utf-8", errors="surrogateescape").read()
    changed = False
    for probe, anchor, ins in EDITS:
        if probe in s:
            continue
        assert anchor in s, "anchor missing in %s: %r" % (path, anchor[:60])
        s = s.replace(anchor, anchor + ins, 1)
        changed = True
    if changed:
        open(path, "w", encoding="utf-8", errors="surrogateescape", newline="").write(s)
    print(("PATCHED " if changed else "already-patched ") + path)

if not ifaces:
    print("NO StoreKit swiftinterface found under", SDK); sys.exit(1)
for p in ifaces:
    patch(p)

# --- also export the new symbols from StoreKit.tbd so they LINK (resolved
# against the on-device StoreKit dylib at runtime; weak via @available). ---
TBD_SYMS = [
    "_$s8StoreKit7ProductV14PurchaseOptionV12winBackOfferyAeC012SubscriptionH0VFZ",
    "_$s8StoreKit7ProductV14PurchaseOptionV16promotionalOffer7offerID9signatureAESS_AC012SubscriptionG0V9SignatureVtFZ",
    "_$s8StoreKit7ProductV16SubscriptionInfoV07RenewalE0V23eligibleWinBackOfferIDsSaySSGvg",
    "_$s8StoreKit7ProductV16SubscriptionInfoV13winBackOffersSayAC0D5OfferVGvg",
    "_$s8StoreKit7ProductV17SubscriptionOfferV0E4TypeV7winBackAGvgZ",
    "_$s8StoreKit7ProductV17SubscriptionOfferV9SignatureV5keyID5nonce9timestamp9signatureAGSS_10Foundation4UUIDVSiAL4DataVtcfC",
    "_$s8StoreKit7ProductV17SubscriptionOfferV9SignatureVMa",
]
tbd = os.path.join(SDK, "System", "Library", "Frameworks", "StoreKit.framework", "StoreKit.tbd")
if os.path.exists(tbd):
    s = open(tbd, encoding="utf-8", errors="surrogateescape").read()
    if TBD_SYMS[0] not in s:
        anchor = ("  - targets:         [ armv7-ios, armv7s-ios, arm64-ios, arm64e-ios ]\n"
                  "    symbols:         [ ")
        ins = "".join("'%s', \n                       " % x for x in TBD_SYMS)
        assert anchor in s, "StoreKit.tbd shared-export anchor not found"
        s = s.replace(anchor, anchor + ins, 1)
        open(tbd, "w", encoding="utf-8", errors="surrogateescape", newline="").write(s)
        print("PATCHED StoreKit.tbd (+%d symbols)" % len(TBD_SYMS))
    else:
        print("already-patched StoreKit.tbd")
