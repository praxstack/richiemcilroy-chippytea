import Darwin
import Foundation
import Security

/// Setup intent, not a claim about the macOS privacy toggle. Full Disk Access
/// requires a user action in System Settings. Opening or enumerating protected
/// folders to test permission can trigger the prompts this flow avoids.
enum DiskAccessPhase: String, Codable, Sendable {
    case intro
    case openingSettings
    case waiting
    case starting
}

/// Apple's nonprompting access check tests these intended scan locations, not
/// the global Full Disk Access toggle. No protected sentinel or TCC data is read.
/// Missing optional folders are harmless; denied or unavailable ones stop setup.
enum HomeFolderAccess {
    static func blockedLocations(in home: URL) -> [String] {
        ["Desktop", "Documents", "Downloads"].filter { name in
            let path = home.appendingPathComponent(name, isDirectory: true).path
            return path.withCString {
                Darwin.access($0, R_OK | X_OK) != 0 && errno != ENOENT
            }
        }
    }

    static func message(for locations: [String]) -> String {
        "macOS still blocks \(locations.joined(separator: ", ")). Enable this copy of chippytea in Full Disk Access, then quit and reopen it. If chippytea is already listed, remove the old entry and add this copy again."
    }
}

/// An attestation belongs to this app installation and code requirement. In
/// particular, replacing an ad-hoc development build can invalidate its TCC
/// grant. Read signing metadata, never protected user folders, before resuming.
struct DiskAccessAppIdentity: Codable, Equatable, Sendable {
    let path: String
    let requirement: String

    static func current() -> DiskAccessAppIdentity? {
        var code: SecCode?
        var staticCode: SecStaticCode?
        var requirement: SecRequirement?
        var text: CFString?
        guard SecCodeCopySelf([], &code) == errSecSuccess, let code,
              SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess, let staticCode,
              SecCodeCopyDesignatedRequirement(staticCode, [], &requirement) == errSecSuccess, let requirement,
              SecRequirementCopyString(requirement, [], &text) == errSecSuccess, let text else { return nil }
        return DiskAccessAppIdentity(path: Bundle.main.bundleURL.path, requirement: text as String)
    }
}
