# Creates the Android release signing key for Vision. Run it ONCE, yourself.
#
#   powershell -ExecutionPolicy Bypass -File scripts\make-android-key.ps1
#
# - You choose the password (typed here, never stored in git).
# - The key goes OUTSIDE the project: %USERPROFILE%\BURGER-OS-signing\burger-os-release.jks
# - src-tauri\gen\android\keystore.properties (git-ignored) tells Gradle where it is.
#
# KEEP A COPY of the .jks file and its password somewhere safe (USB stick, password manager).
# Every update of the app must be signed with this same key; if it is lost, tablets can only
# install a new version after uninstalling the old one (and losing that device's data).
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $env:JAVA_HOME) { $env:JAVA_HOME = "C:\Program Files\Android\Android Studio\jbr" }
$keytool = Join-Path $env:JAVA_HOME "bin\keytool.exe"
if (-not (Test-Path $keytool)) { throw "keytool not found: set JAVA_HOME to a JDK (Android Studio's jbr folder)" }

$dir = Join-Path $env:USERPROFILE "BURGER-OS-signing"
$jks = Join-Path $dir "burger-os-release.jks"
$props = Join-Path $root "src-tauri\gen\android\keystore.properties"
if (Test-Path $jks) { throw "A key already exists at $jks. Keep using it (delete it only if you really mean to start over)." }
New-Item -ItemType Directory -Force $dir | Out-Null

$p1 = Read-Host "Choose a password for the signing key (at least 8 characters)" -AsSecureString
$p2 = Read-Host "Type it again" -AsSecureString
$plain = [Runtime.InteropServices.Marshal]::PtrToStringAuto([Runtime.InteropServices.Marshal]::SecureStringToBSTR($p1))
$again = [Runtime.InteropServices.Marshal]::PtrToStringAuto([Runtime.InteropServices.Marshal]::SecureStringToBSTR($p2))
if ($plain -ne $again) { throw "The passwords don't match." }
if ($plain.Length -lt 8) { throw "Use at least 8 characters." }
$shop = Read-Host "Shop name (goes in the certificate, e.g. Burger Lab Casa)"
if (-not $shop) { $shop = "Vision" }

& $keytool -genkeypair -v -keystore $jks -alias burger-os -keyalg RSA -keysize 3072 -validity 10000 `
  -storepass $plain -keypass $plain -dname "CN=$shop, O=$shop, C=MA"
if ($LASTEXITCODE) { throw "keytool failed" }

# Gradle reads this file; forward slashes work on Windows.
@"
storeFile=$($jks.Replace('\', '/'))
storePassword=$plain
keyAlias=burger-os
keyPassword=$plain
"@ | Set-Content -Encoding ascii $props

Write-Host ""
Write-Host "Done. Key: $jks"
Write-Host "Back it up now (copy the whole folder $dir) and keep the password safe."
Write-Host "Then build the signed APK:  powershell -ExecutionPolicy Bypass -File scripts\build-android.ps1"
