import libcst as cst

class Web3V7Transformer(cst.CSTTransformer):
    def __init__(self):
        super().__init__()
        self.requires_exception_import = False
        self.requires_middleware_import = False

    # 1. Replace keyword arguments (e.g., w3.eth.get_logs(fromBlock=1))
    def leave_Arg(self, original_node: cst.Arg, updated_node: cst.Arg) -> cst.Arg:
        if updated_node.keyword:
            kw = updated_node.keyword.value
            arg_map = {"fromBlock": "from_block", "toBlock": "to_block", "block_hash": "block_hash"}
            if kw in arg_map:
                return updated_node.with_changes(keyword=cst.Name(arg_map[kw]))
        return updated_node

    # 2. Replace keys in dictionaries (e.g., {"fromBlock": 1})
    def leave_DictElement(self, original_node: cst.DictElement, updated_node: cst.DictElement) -> cst.DictElement:
        if isinstance(updated_node.key, cst.SimpleString):
            raw_key = updated_node.key.value.strip("\"'")
            arg_map = {
                "fromBlock": "from_block",
                "toBlock": "to_block",
                "blockHash": "block_hash"
            }
            if raw_key in arg_map:
                new_key_str = f'"{arg_map[raw_key]}"'
                return updated_node.with_changes(key=cst.SimpleString(new_key_str))
        return updated_node

    # 3. Replace attribute access (e.g., middlewares -> middleware)
    def leave_Attribute(self, original_node: cst.Attribute, updated_node: cst.Attribute) -> cst.Attribute:
        if updated_node.attr.value == "middlewares":
            return updated_node.with_changes(attr=cst.Name("middleware"))
        return updated_node

    # 4. Replace identifiers (e.g., pythonic_middleware -> PythonicMiddleware)
    def leave_Name(self, original_node: cst.Name, updated_node: cst.Name) -> cst.Name:
        middleware_map = {
            "pythonic_middleware": "PythonicMiddleware",
            "geth_poa_middleware": "ExtraDataToPOAMiddleware",
        }
        if updated_node.value in middleware_map:
            self.requires_middleware_import = True
            return updated_node.with_changes(value=middleware_map[updated_node.value])
        return updated_node

    # 5. Replace exception handlers
    def leave_ExceptHandler(self, original_node: cst.ExceptHandler, updated_node: cst.ExceptHandler) -> cst.ExceptHandler:
        if updated_node.type and isinstance(updated_node.type, cst.Name):
            exc_map = {
                "ValueError": "Web3ValueError", "TypeError": "Web3TypeError",
                "AttributeError": "Web3AttributeError", "AssertionError": "Web3AssertionError",
            }
            if updated_node.type.value in exc_map:
                self.requires_exception_import = True
                return updated_node.with_changes(type=cst.Name(exc_map[updated_node.type.value]))
        return updated_node

    # 6. Intelligent Import insertion
    def leave_Module(self, original_node: cst.Module, updated_node: cst.Module) -> cst.Module:
        if not (self.requires_middleware_import or self.requires_exception_import):
            return updated_node

        new_body = list(updated_node.body)
        insert_index = 0

        for i, stmt in enumerate(new_body):
            if isinstance(stmt, cst.SimpleStatementLine):
                if isinstance(stmt.body[0], cst.Expr) and isinstance(stmt.body[0].value, (cst.SimpleString, cst.ConcatenatedString)):
                    insert_index = i + 1
                    continue
                if isinstance(stmt.body[0], cst.ImportFrom) and getattr(stmt.body[0].module, "value", "") == "__future__":
                    insert_index = i + 1
                    continue
            break

        imports_to_add = []
        if self.requires_middleware_import:
            imports_to_add.append(cst.parse_statement("from web3.middleware import PythonicMiddleware, ExtraDataToPOAMiddleware\n"))
        if self.requires_exception_import:
            imports_to_add.append(cst.parse_statement("from web3.exceptions import Web3ValueError, Web3TypeError, Web3AttributeError, Web3AssertionError\n"))

        for imp in reversed(imports_to_add):
            new_body.insert(insert_index, imp)

        return updated_node.with_changes(body=new_body)

# Codemod Registry 标准入口函数
def transform(source_code: str) -> str:
    try:
        tree = cst.parse_module(source_code)
        transformer = Web3V7Transformer()
        modified_tree = tree.visit(transformer)
        return modified_tree.code
    except Exception:
        # 如果解析失败（例如代码本身有语法错误），原样返回以防止中断整个执行流
        return source_code
