#!/usr/bin/env python3
"""Dependency-free, deterministic Xcode project generation."""
from pathlib import Path
import hashlib
import json

ROOT = Path(__file__).resolve().parent.parent
IOS = ROOT / 'ios'
PROJECT = IOS / 'MeshPay.xcodeproj'
PROJECT.mkdir(exist_ok=True)
objects = {}

def ident(name):
    return hashlib.sha256(name.encode()).hexdigest()[:24].upper()

def add(name, content):
    key = ident(name)
    objects[key] = content
    return key

def q(s):
    return json.dumps(str(s))

def settings(values):
    return '{' + ' '.join(f'{key} = {q(value)};' for key, value in values.items()) + '}'

def configs(name, values):
    refs = []
    for mode in ['Debug', 'Release']:
        configured = dict(values)
        if mode == 'Debug':
            configured.update(SWIFT_OPTIMIZATION_LEVEL='-Onone', SWIFT_ACTIVE_COMPILATION_CONDITIONS='DEBUG', DEBUG_INFORMATION_FORMAT='dwarf')
        else:
            configured.update(SWIFT_OPTIMIZATION_LEVEL='-O', DEBUG_INFORMATION_FORMAT='dwarf-with-dsym')
        refs.append(add(name + mode, f'isa = XCBuildConfiguration; name = {mode}; buildSettings = {settings(configured)};'))
    return add(name + 'configlist', f'isa = XCConfigurationList; buildConfigurations = ({",".join(refs)},); defaultConfigurationIsVisible = 0; defaultConfigurationName = Release;')

app_sources, test_sources, file_refs = [], [], []
for directory, builds in [('MeshPay', app_sources), ('MeshPayTests', test_sources)]:
    for path in sorted((IOS / directory).glob('*.swift')):
        relative = str(path.relative_to(IOS))
        ref = add(relative, f'isa = PBXFileReference; lastKnownFileType = sourcecode.swift; path = {q(relative)}; sourceTree = "<group>";')
        file_refs.append(ref)
        builds.append(add(relative + 'build', f'isa = PBXBuildFile; fileRef = {ref};'))

framework = add('framework', 'isa = PBXFileReference; lastKnownFileType = wrapper.xcframework; path = Frameworks/MeshCore.xcframework; sourceTree = "<group>";')
framework_build = add('frameworkbuild', f'isa = PBXBuildFile; fileRef = {framework};')
app_product = add('app', 'isa = PBXFileReference; explicitFileType = wrapper.application; path = MeshPay.app; sourceTree = BUILT_PRODUCTS_DIR;')
test_product = add('tests', 'isa = PBXFileReference; explicitFileType = wrapper.cfbundle; path = MeshPayTests.xctest; sourceTree = BUILT_PRODUCTS_DIR;')
products = add('products', f'isa = PBXGroup; children = ({app_product},{test_product},); name = Products; sourceTree = "<group>";')
main_group = add('group', f'isa = PBXGroup; children = ({",".join(file_refs + [framework,products])},); sourceTree = "<group>";')
linkkit = add('linkkit', 'isa = PBXFileReference; lastKnownFileType = wrapper.xcframework; path = Frameworks/LinkKit.xcframework; sourceTree = "<group>";')
linkkit_build = add('linkkitbuild', f'isa = PBXBuildFile; fileRef = {linkkit};')
linkkit_embed = add('linkkitembed', f'isa = PBXBuildFile; fileRef = {linkkit}; settings = {{ ATTRIBUTES = (CodeSignOnCopy,RemoveHeadersOnCopy,); }};')
linkkit_embed_phase = add('embedframeworks', f'isa = PBXCopyFilesBuildPhase; buildActionMask = 2147483647; dstPath = ""; dstSubfolderSpec = 10; files = ({linkkit_embed},); name = "Embed Frameworks"; runOnlyForDeploymentPostprocessing = 0;')
app_config = configs('app', {
    'PRODUCT_BUNDLE_IDENTIFIER': '$(MESH_APP_BUNDLE_ID)', 'PRODUCT_NAME': 'MeshPay', 'SWIFT_VERSION': '5.0',
    'IPHONEOS_DEPLOYMENT_TARGET': '17.0', 'SDKROOT': 'iphoneos', 'ARCHS': 'arm64', 'TARGETED_DEVICE_FAMILY': '1',
    'SUPPORTED_PLATFORMS': 'iphoneos iphonesimulator', 'SWIFT_OBJC_BRIDGING_HEADER': 'MeshPay/MeshCore.h',
    'INFOPLIST_FILE': 'MeshPay/Info.plist', 'CODE_SIGN_ENTITLEMENTS': '$(MESH_ENTITLEMENTS)',
    'CODE_SIGN_STYLE': 'Automatic', 'ENABLE_USER_SCRIPT_SANDBOXING': 'NO',
    'LD_RUNPATH_SEARCH_PATHS': '$(inherited) @executable_path/Frameworks',
    'OTHER_LDFLAGS': '$(inherited) -lc++ -liconv -lresolv -framework Security -framework SystemConfiguration',
    'ASSETCATALOG_COMPILER_APPICON_NAME': 'AppIcon',
})
test_config = configs('uitests', {
    'PRODUCT_BUNDLE_IDENTIFIER': '$(MESH_APP_BUNDLE_ID).uitests', 'PRODUCT_NAME': 'MeshPayTests', 'SWIFT_VERSION': '5.0',
    'IPHONEOS_DEPLOYMENT_TARGET': '17.0', 'SDKROOT': 'iphoneos', 'ARCHS': 'arm64', 'TARGETED_DEVICE_FAMILY': '1',
    'SUPPORTED_PLATFORMS': 'iphoneos iphonesimulator', 'GENERATE_INFOPLIST_FILE': 'YES', 'TEST_TARGET_NAME': 'MeshPay',
    'CODE_SIGN_STYLE': 'Automatic',
})
project_config = configs('project', {'MESH_APP_BUNDLE_ID': 'app.mesh.sandbox', 'MESH_ENTITLEMENTS': 'MeshPay/MeshPay.entitlements', 'CLANG_ENABLE_MODULES': 'YES', 'SWIFT_VERSION': '5.0', 'IPHONEOS_DEPLOYMENT_TARGET': '17.0'})

def phase(name, kind, files):
    listed = ','.join(files) + (',' if files else '')
    return add(name, f'isa = {kind}; buildActionMask = 2147483647; files = ({listed}); runOnlyForDeploymentPostprocessing = 0;')

app_source_phase = phase('appsources', 'PBXSourcesBuildPhase', app_sources)
app_framework_phase = phase('appframeworks', 'PBXFrameworksBuildPhase', [framework_build,linkkit_build])
asset_ref = add('assets', 'isa = PBXFileReference; lastKnownFileType = folder.assetcatalog; path = MeshPay/Assets.xcassets; sourceTree = "<group>";')
asset_build = add('assetsbuild', f'isa = PBXBuildFile; fileRef = {asset_ref};')
app_resource_phase = phase('appresources', 'PBXResourcesBuildPhase', [asset_build])
test_source_phase = phase('testsources', 'PBXSourcesBuildPhase', test_sources)
test_framework_phase = phase('testframeworks', 'PBXFrameworksBuildPhase', [])
test_resource_phase = phase('testresources', 'PBXResourcesBuildPhase', [])
app_target = add('apptarget', f'isa = PBXNativeTarget; buildConfigurationList = {app_config}; buildPhases = ({app_source_phase},{app_framework_phase},{app_resource_phase},{linkkit_embed_phase},); buildRules = (); dependencies = (); name = MeshPay; packageProductDependencies = (); productName = MeshPay; productReference = {app_product}; productType = "com.apple.product-type.application";')
root_id = ident('projectroot')
proxy = add('proxy', f'isa = PBXContainerItemProxy; containerPortal = {root_id}; proxyType = 1; remoteGlobalIDString = {app_target}; remoteInfo = MeshPay;')
dependency = add('dependency', f'isa = PBXTargetDependency; target = {app_target}; targetProxy = {proxy};')
test_target = add('testtarget', f'isa = PBXNativeTarget; buildConfigurationList = {test_config}; buildPhases = ({test_source_phase},{test_framework_phase},{test_resource_phase},); buildRules = (); dependencies = ({dependency},); name = MeshPayTests; productName = MeshPayTests; productReference = {test_product}; productType = "com.apple.product-type.bundle.ui-testing";')
add('projectroot', f'isa = PBXProject; attributes = {{ LastUpgradeCheck = 2600; BuildIndependentTargetsInParallel = 1; TargetAttributes = {{ {test_target} = {{ TestTargetID = {app_target}; }}; }}; }}; buildConfigurationList = {project_config}; compatibilityVersion = "Xcode 14.0"; developmentRegion = en; hasScannedForEncodings = 0; knownRegions = (en,Base,); mainGroup = {main_group}; packageReferences = (); productRefGroup = {products}; projectDirPath = ""; projectRoot = ""; targets = ({app_target},{test_target},);')
text = '// !$*UTF8*$!\n{ archiveVersion = 1; classes = {}; objectVersion = 56; objects = {\n'
text += '\n'.join(f'{key} = {{{value}}};' for key,value in objects.items())
text += f'\n}}; rootObject = {root_id}; }}\n'
(PROJECT / 'project.pbxproj').write_text(text)
schemes = PROJECT / 'xcshareddata' / 'xcschemes'
schemes.mkdir(parents=True, exist_ok=True)
ref = lambda key, name: f'<BuildableReference BuildableIdentifier="primary" BlueprintIdentifier="{key}" BuildableName="{name}" BlueprintName="{name.split(".")[0]}" ReferencedContainer="container:MeshPay.xcodeproj"/>'
scheme = f'''<?xml version="1.0" encoding="UTF-8"?>
<Scheme LastUpgradeVersion="2600" version="1.3">
<BuildAction parallelizeBuildables="YES" buildImplicitDependencies="YES"><BuildActionEntries><BuildActionEntry buildForTesting="YES" buildForRunning="YES" buildForProfiling="YES" buildForArchiving="YES" buildForAnalyzing="YES">{ref(app_target,'MeshPay.app')}</BuildActionEntry></BuildActionEntries></BuildAction>
<TestAction buildConfiguration="Debug" selectedDebuggerIdentifier="Xcode.DebuggerFoundation.Debugger.LLDB" selectedLauncherIdentifier="Xcode.IDEFoundation.Launcher.LLDB" shouldUseLaunchSchemeArgsEnv="YES"><Testables><TestableReference skipped="NO">{ref(test_target,'MeshPayTests.xctest')}</TestableReference></Testables></TestAction>
<LaunchAction buildConfiguration="Debug" selectedDebuggerIdentifier="Xcode.DebuggerFoundation.Debugger.LLDB" selectedLauncherIdentifier="Xcode.IDEFoundation.Launcher.LLDB" launchStyle="0" useCustomWorkingDirectory="NO" ignoresPersistentStateOnLaunch="NO" debugDocumentVersioning="YES" debugServiceExtension="internal" allowLocationSimulation="YES"><BuildableProductRunnable runnableDebuggingMode="0">{ref(app_target,'MeshPay.app')}</BuildableProductRunnable></LaunchAction>
<ProfileAction buildConfiguration="Release" shouldUseLaunchSchemeArgsEnv="YES" savedToolIdentifier="" useCustomWorkingDirectory="NO" debugDocumentVersioning="YES"><BuildableProductRunnable runnableDebuggingMode="0">{ref(app_target,'MeshPay.app')}</BuildableProductRunnable></ProfileAction>
<AnalyzeAction buildConfiguration="Debug"/><ArchiveAction buildConfiguration="Release" revealArchiveInOrganizer="YES"/>
</Scheme>'''
(schemes / 'MeshPay.xcscheme').write_text(scheme)
print(PROJECT)
