//! TLS-проба: валиден ли сертификат, сколько дней до истечения, версия протокола (B04, O05).

use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TlsInfo {
    /// Сертификат прошёл проверку цепочки и имени.
    pub valid: bool,
    pub error: Option<String>,
    /// Окончание действия, unix-время.
    pub not_after: Option<i64>,
    pub days_left: Option<i64>,
    pub issuer: Option<String>,
    /// «TLS 1.3», «TLS 1.2».
    pub protocol: Option<String>,
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn verified_config() -> ClientConfig {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("протоколы rustls по умолчанию")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// Верификатор без проверки: нужен, только чтобы прочитать даты невалидного сертификата.
#[derive(Debug)]
struct AcceptAny(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn insecure_config() -> ClientConfig {
    let p = provider();
    ClientConfig::builder_with_provider(p.clone())
        .with_safe_default_protocol_versions()
        .expect("протоколы rustls по умолчанию")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAny(p)))
        .with_no_client_auth()
}

async fn handshake(host: &str, cfg: ClientConfig) -> Result<(Option<Vec<u8>>, Option<String>), String> {
    let tcp = timeout(Duration::from_secs(8), TcpStream::connect((host, 443)))
        .await
        .map_err(|_| "таймаут подключения".to_string())?
        .map_err(|e| format!("подключение: {e}"))?;
    let name = ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
    let stream = timeout(Duration::from_secs(8), TlsConnector::from(Arc::new(cfg)).connect(name, tcp))
        .await
        .map_err(|_| "таймаут TLS-рукопожатия".to_string())?
        .map_err(|e| e.to_string())?;
    let (_, session) = stream.get_ref();
    let der = session.peer_certificates().and_then(|c| c.first()).map(|c| c.as_ref().to_vec());
    let proto = session.protocol_version().map(|v| match format!("{v:?}").as_str() {
        "TLSv1_3" => "TLS 1.3".to_string(),
        "TLSv1_2" => "TLS 1.2".to_string(),
        other => other.to_string(),
    });
    Ok((der, proto))
}

fn fill_cert(info: &mut TlsInfo, der: &[u8]) {
    if let Ok((_, cert)) = x509_parser::parse_x509_certificate(der) {
        let na = cert.validity().not_after.timestamp();
        info.not_after = Some(na);
        info.days_left = Some((na - chrono::Utc::now().timestamp()).div_euclid(86_400));
        info.issuer = cert
            .issuer()
            .iter_organization()
            .next()
            .and_then(|a| a.as_str().ok())
            .map(str::to_string)
            .or_else(|| Some(cert.issuer().to_string()));
    }
}

/// Имена из сертификата: SAN, а без них — CN.
fn cert_names(der: &[u8]) -> Vec<String> {
    let Ok((_, cert)) = x509_parser::parse_x509_certificate(der) else { return Vec::new() };
    let mut names: Vec<String> = cert
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|ext| {
            ext.value
                .general_names
                .iter()
                .filter_map(|g| match g {
                    x509_parser::extensions::GeneralName::DNSName(n) => Some(n.to_lowercase()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    if names.is_empty() {
        names.extend(cert.subject().iter_common_name().filter_map(|c| c.as_str().ok()).map(str::to_lowercase));
    }
    names
}

/// Имя сертификата подходит хосту: точно или по маске «*.example.com» на один уровень.
fn name_matches(name: &str, host: &str) -> bool {
    let host = host.to_lowercase();
    match name.strip_prefix("*.") {
        Some(base) => host.split_once('.').is_some_and(|(_, rest)| rest == base),
        None => name == host,
    }
}

pub async fn probe(host: &str) -> TlsInfo {
    let mut info = TlsInfo::default();
    match handshake(host, verified_config()).await {
        Ok((der, proto)) => {
            info.valid = true;
            info.protocol = proto;
            if let Some(d) = der {
                fill_cert(&mut info, &d);
            }
        }
        Err(e) => {
            let mut names = Vec::new();
            if let Ok((der, proto)) = handshake(host, insecure_config()).await {
                info.protocol = proto;
                if let Some(d) = der {
                    fill_cert(&mut info, &d);
                    names = cert_names(&d);
                }
            }
            // rustls сообщает UnknownIssuer раньше, чем проверяет срок и имя: сначала смотрим сам
            // сертификат (barazza.by: выдан на *.dadapro.com и истёк — браузер покажет ошибку).
            let expired = info.days_left.is_some_and(|d| d < 0);
            let wrong_name = !names.is_empty() && !names.iter().any(|n| name_matches(n, host));
            info.error = Some(if expired {
                format!("сертификат истёк {} дн. назад — браузер покажет ошибку", -info.days_left.unwrap_or(0))
            } else if wrong_name {
                format!("сертификат выдан на другой домен ({}) — браузер покажет ошибку", names.iter().take(2).cloned().collect::<Vec<_>>().join(", "))
            } else if e.contains("UnknownIssuer") {
                // Неполная цепочка: браузеры достраивают её сами (AIA, кэш), а боты, curl и
                // большинство библиотек — нет.
                "неполная цепочка сертификатов: браузеры обычно открывают сайт, а поисковые и ИИ-боты могут не открыть".to_string()
            } else {
                e
            });
        }
    }
    info
}

#[cfg(test)]
mod tests {
    #[test]
    fn certificate_names_match_hosts() {
        // barazza.by с сертификатом *.dadapro.com — чужое имя; маска покрывает один уровень.
        assert!(!super::name_matches("*.dadapro.com", "barazza.by"));
        assert!(super::name_matches("*.example.com", "www.example.com"));
        assert!(!super::name_matches("*.example.com", "a.b.example.com"));
        assert!(super::name_matches("example.com", "EXAMPLE.com"));
    }
}
