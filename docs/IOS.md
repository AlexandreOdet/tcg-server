# App iOS : plan de départ

Périmètre v1 de l'app, aligné sur le backend : **connexion Apple**, **ouverture de boosters**,
**collection**. Duels, échanges, premium et packs viennent après. Le contrat d'API est dans
[`API.md`](API.md).

> ⚠️ Les extraits Swift de ce document n'ont **pas été compilés** (ils ont été écrits dans un
> environnement sans Xcode). Ils illustrent la forme à adopter ; vérifie-les dans Xcode.

## Prérequis Apple Developer

- Compte Apple Developer (99 €/an) pour la capability Sign in with Apple et TestFlight.
- App ID avec la capability **Sign in with Apple** activée.
- Le **bundle ID** de l'app doit être strictement celui configuré côté serveur dans
  `APPLE_BUNDLE_ID` : c'est l'audience (`aud`) que le serveur exige dans le jeton Apple.
- Sign in with Apple ne fonctionne pas toujours bien sur simulateur sans compte iCloud connecté :
  prévois de tester sur un appareil réel.

## Choix techniques proposés

| Sujet | Proposition | Pourquoi |
|---|---|---|
| Cible minimale | iOS 17 | `@Observable`, SwiftData |
| UI | SwiftUI | Choix du projet |
| Architecture | MVVM léger (`@Observable`) | Simple, suffisant pour la v1 |
| Réseau | `URLSession` + `actor` | Pas de dépendance, refresh sérialisé |
| Jetons | Keychain | Jamais `UserDefaults` |
| Collection en cache | SwiftData (ou GRDB) | Affichage instantané, hors ligne en lecture |
| Images | Cache dédié (ex. Nuke) | `AsyncImage` ne met pas assez en cache |
| Client d'API | Écrit à la main pour l'instant, puis généré depuis l'OpenAPI (phase 4) | Le contrat est petit |

Organisation conseillée : modules Swift Package (`Networking`, `Auth`, `Collection`, `UIComponents`)
plutôt qu'un seul gros target, pour garder les dépendances claires et les tests rapides.

## Connexion Sign in with Apple

Le point délicat est le **nonce** (protection contre le rejeu de jeton) :

1. L'app génère un nonce **brut** aléatoire.
2. Elle envoie à Apple son **SHA-256 en hexadécimal minuscule** (`request.nonce`).
3. Elle envoie au serveur le jeton d'identité reçu **et le nonce brut**.
4. Le serveur recalcule le SHA-256 et le compare au claim `nonce` du jeton.

```swift
import AuthenticationServices
import CryptoKit

enum Nonce {
    static func random(byteCount: Int = 32) -> String {
        var bytes = [UInt8](repeating: 0, count: byteCount)
        precondition(SecRandomCopyBytes(kSecRandomDefault, byteCount, &bytes) == errSecSuccess)
        return bytes.map { String(format: "%02x", $0) }.joined()
    }

    static func sha256Hex(_ input: String) -> String {
        SHA256.hash(data: Data(input.utf8)).map { String(format: "%02x", $0) }.joined()
    }
}

// Dans la vue de connexion
@State private var rawNonce = ""

SignInWithAppleButton(.signIn) { request in
    rawNonce = Nonce.random()
    request.requestedScopes = [.email]        // le pseudo sera choisi dans l'app
    request.nonce = Nonce.sha256Hex(rawNonce) // jamais le nonce brut
} onCompletion: { result in
    guard case .success(let auth) = result,
          let credential = auth.credential as? ASAuthorizationAppleIDCredential,
          let tokenData = credential.identityToken,
          let identityToken = String(data: tokenData, encoding: .utf8) else { return }
    Task { await viewModel.signIn(identityToken: identityToken, nonce: rawNonce) }
}
```

Points d'attention :

- Apple ne renvoie le nom et l'e-mail **qu'à la première autorisation** (le serveur conserve l'e-mail
  connu et n'écrase jamais avec une valeur absente). On ne demande que `.email` : le pseudo est choisi
  par le joueur.
- Si l'utilisateur révoque l'app dans ses réglages Apple, `ASAuthorizationAppleIDProvider
  .getCredentialState(forUserID:)` le détecte : prévoir de le vérifier au lancement.

## Stockage et rafraîchissement des jetons

- Stocker `access_token` et `refresh_token` dans le **Keychain**
  (`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly` pour que l'app puisse rafraîchir en arrière-plan,
  sans migrer vers un autre appareil).
- **Le refresh token est à usage unique** : chaque `/auth/refresh` en renvoie un nouveau, à écrire dans le
  Keychain *avant* de rejouer la requête échouée.
- **Un seul refresh à la fois** : sinon deux requêtes concurrentes rejouent le même jeton. Le serveur tolère
  10 s de course, puis révoque toute la session. Il faut donc sérialiser côté app :

```swift
actor SessionManager {
    private let store: SessionStore            // Keychain
    private let api: AuthAPI
    private var refreshTask: Task<Session, Error>?

    /// À appeler quand une requête a reçu un 401 avec `failedAccessToken`.
    func refreshed(after failedAccessToken: String) async throws -> Session {
        // Un autre appel a déjà rafraîchi pendant qu'on attendait : on réutilise son résultat.
        if let current = try store.load(), current.accessToken != failedAccessToken {
            return current
        }
        if let task = refreshTask { return try await task.value }

        let task = Task { () -> Session in
            guard let current = try store.load() else { throw AuthError.signedOut }
            let renewed = try await api.refresh(current.refreshToken)
            try store.save(renewed)            // avant de rejouer quoi que ce soit
            return renewed
        }
        refreshTask = task
        defer { refreshTask = nil }
        do {
            return try await task.value
        } catch AuthError.unauthorized {        // 401 sur /auth/refresh : session terminée
            try? store.clear()
            throw AuthError.signedOut           // l'UI repasse par l'écran de connexion
        }
    }
}
```

Règle côté client : un **401 sur une requête normale** déclenche *un* refresh puis *un* rejeu ; un
**401 sur `/auth/refresh`** signifie « session terminée », sans nouvelle tentative.

Ne pas supposer que tout 4xx contient un corps `error` (voir `API.md`).

## Écrans de la v1

1. **Connexion** : bouton Sign in with Apple, états de chargement et d'erreur.
2. **Choix du pseudo** (si `is_new_user`) : nécessite la route `PUT /me`, pas encore écrite.
3. **Accueil / boosters** : stock disponible, compte à rebours avant le prochain booster (phase 3).
4. **Ouverture d'un booster** : animation de révélation des 5 cartes. Le tirage est fait par le serveur
   *avant* l'animation : elle est purement visuelle.
5. **Collection** : grille filtrable par rareté, détail d'une carte (titre, extrait, lien vers l'article
   Wikipédia, attribution CC BY-SA).
6. **Réglages** : déconnexion, suppression de compte (obligatoire pour l'App Store), mentions légales.

## Ordre de travail suggéré

1. Projet Xcode, bundle ID, capability Sign in with Apple, configuration de l'URL du serveur par
   environnement (debug : machine locale, release : production).
2. `Networking` : client, gestion d'erreurs, format `{ error: { code, message } }`.
3. `Auth` : flux Sign in with Apple, Keychain, `SessionManager`, tests unitaires du refresh
   concurrent (c'est le plus fragile).
4. Écran de connexion + `GET /me` pour valider toute la chaîne **contre le serveur réel**.
5. Ensuite, boosters et collection quand la phase 3 du serveur est prête.

## À prévoir pour l'App Store (dès le début, pas à la fin)

- Suppression de compte **dans l'app** (guideline 5.1.1(v)), avec révocation du token Apple côté
  serveur (pas encore implémentée).
- Politique de confidentialité et déclaration de collecte de données.
- Attribution Wikipédia (CC BY-SA) visible dans l'app et lien vers chaque article.
- Classification d'âge : le contenu de Wikipédia n'est pas filtré à ce stade (voir `PLAN.md`).
