#!/usr/bin/env python3
"""
Direct API test for Hunyuan AI3D
"""

import json
import time
import hmac
import hashlib
import requests
from datetime import datetime
import os

# 配置
SECRET_ID = os.environ.get("TENCENTCLOUD_SECRET_ID")
SECRET_KEY = os.environ.get("TENCENTCLOUD_SECRET_KEY")
ENDPOINT = "ai3d.tencentcloudapi.com"
REGION = "ap-shanghai"
SERVICE = "ai3d"
VERSION = "2025-05-13"

def sign_tc3(secret_key, date, service, string_to_sign):
    """TC3-HMAC-SHA256 签名"""
    def sign(key, msg):
        return hmac.new(key, msg.encode("utf-8"), hashlib.sha256).digest()
    
    secret_date = sign(("TC3" + secret_key).encode("utf-8"), date)
    secret_service = sign(secret_date, service)
    secret_signing = sign(secret_service, "tc3_request")
    signature = hmac.new(secret_signing, string_to_sign.encode("utf-8"), hashlib.sha256).hexdigest()
    return signature

def call_api(action, params):
    """调用腾讯云API"""
    timestamp = int(time.time())
    date = datetime.utcfromtimestamp(timestamp).strftime("%Y-%m-%d")
    
    # 构建请求
    headers = {
        "Host": ENDPOINT,
        "Content-Type": "application/json",
        "X-TC-Action": action,
        "X-TC-Version": VERSION,
        "X-TC-Region": REGION,
        "X-TC-Timestamp": str(timestamp),
    }
    
    payload = json.dumps(params)
    
    # 计算签名
    canonical_headers = "\n".join([f"{k.lower()}:{v}" for k, v in sorted(headers.items())])
    signed_headers = ";".join([k.lower() for k in sorted(headers.keys())])
    hashed_payload = hashlib.sha256(payload.encode("utf-8")).hexdigest()
    
    canonical_request = f"POST\n/\n\n{canonical_headers}\n\n{signed_headers}\n{hashed_payload}"
    
    credential_scope = f"{date}/{SERVICE}/tc3_request"
    string_to_sign = f"TC3-HMAC-SHA256\n{timestamp}\n{credential_scope}\n{hashlib.sha256(canonical_request.encode('utf-8')).hexdigest()}"
    
    signature = sign_tc3(SECRET_KEY, date, SERVICE, string_to_sign)
    
    headers["Authorization"] = f"TC3-HMAC-SHA256 Credential={SECRET_ID}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}"
    
    # 发送请求
    response = requests.post(f"https://{ENDPOINT}", headers=headers, data=payload)
    return response.json()

def test_standard_api():
    """测试 Standard API"""
    print("Testing Standard API (SubmitHunyuanTo3DJob)...")
    
    # 提交任务
    submit_params = {
        "Prompt": "一个简单的立方体"
    }
    
    result = call_api("SubmitHunyuanTo3DJob", submit_params)
    print(f"Submit result: {json.dumps(result, indent=2, ensure_ascii=False)}")
    
    if "Response" in result and "JobId" in result["Response"]:
        job_id = result["Response"]["JobId"]
        print(f"\nJob ID: {job_id}")
        
        # 查询状态
        print("\nQuerying job status...")
        for i in range(5):
            time.sleep(3)
            query_result = call_api("QueryHunyuanTo3DJob", {"JobId": job_id})
            print(f"Query #{i+1}: {json.dumps(query_result, indent=2, ensure_ascii=False)}")
            
            if "Response" in query_result:
                status = query_result["Response"].get("Status", "")
                if status.lower() in ["done", "success", "completed", "failed", "error"]:
                    break
    
    return result

def test_pro_api():
    """测试 Professional API"""
    print("\nTesting Professional API (SubmitHunyuanTo3DProJob)...")
    
    submit_params = {
        "Prompt": "一个简单的球体",
        "EnablePBR": True,
        "FaceCount": 50000
    }
    
    result = call_api("SubmitHunyuanTo3DProJob", submit_params)
    print(f"Submit result: {json.dumps(result, indent=2, ensure_ascii=False)}")
    
    if "Response" in result and "JobId" in result["Response"]:
        job_id = result["Response"]["JobId"]
        print(f"\nJob ID: {job_id}")
        
        # 查询状态
        print("\nQuerying job status...")
        for i in range(3):
            time.sleep(3)
            query_result = call_api("QueryHunyuanTo3DProJob", {"JobId": job_id})
            print(f"Query #{i+1}: {json.dumps(query_result, indent=2, ensure_ascii=False)}")

def test_rapid_api():
    """测试 Rapid API"""
    print("\nTesting Rapid API (SubmitHunyuanTo3DRapidJob)...")
    
    submit_params = {
        "Prompt": "一个简单的圆锥",
        "ResultFormat": "OBJ"
    }
    
    result = call_api("SubmitHunyuanTo3DRapidJob", submit_params)
    print(f"Submit result: {json.dumps(result, indent=2, ensure_ascii=False)}")
    
    if "Response" in result and "JobId" in result["Response"]:
        job_id = result["Response"]["JobId"]
        print(f"\nJob ID: {job_id}")
        
        # 查询状态
        print("\nQuerying job status...")
        for i in range(3):
            time.sleep(3)
            query_result = call_api("QueryHunyuanTo3DRapidJob", {"JobId": job_id})
            print(f"Query #{i+1}: {json.dumps(query_result, indent=2, ensure_ascii=False)}")

if __name__ == "__main__":
    if not SECRET_ID or not SECRET_KEY:
        print("Error: Please set TENCENTCLOUD_SECRET_ID and TENCENTCLOUD_SECRET_KEY environment variables")
        exit(1)
    
    print(f"Using Secret ID: {SECRET_ID[:10]}...")
    print(f"Using endpoint: {ENDPOINT}")
    print(f"Region: {REGION}")
    print(f"Version: {VERSION}")
    print("-" * 50)
    
    # 测试各个 API
    try:
        # test_standard_api()
        test_pro_api()
        # test_rapid_api()
    except Exception as e:
        print(f"Error: {e}")
        import traceback
        traceback.print_exc()
