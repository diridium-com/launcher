# Windows Build Prerequisite: OpenSSL

## Clone Microsoft VCPKG and use it to install OpenSSL

```powershell
git clone https://github.com/microsoft/vcpkg.git "C:\vcpkg"
cd "C:\vcpkg"
.\bootstrap-vcpkg.bat
.\vcpkg.exe install openssl --triplet x64-windows
```

## Set Environment Variables:

OPENSSL_DIR
C:\vcpkg\installed\x64-windows

OPENSSL_LIB_DIR
C:\vcpkg\installed\x64-windows\lib

OPENSSL_INCLUDE_DIR
C:\vcpkg\installed\x64-windows\include

OPENSSL_LIB_DIR
C:\Program Files\OpenSSL-Win64\lib

OPENSSL_NO_VENDOR
1

##Add to PATH:
C:\vcpkg\installed\x64-windows\bin
